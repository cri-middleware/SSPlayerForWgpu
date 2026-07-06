# SSPlayer for wgpu — 実装正典（Cold Start）

> **この文書の位置づけ**
> `ssplayer-wgpu`（wgpu ベースの SpriteStudio 7 プレイヤー）の**唯一の正典**。設計判断・アーキテクチャ・実装フェーズ・検証手順を一箇所に集約する。コード派が上から読めば着手できる粒度で書く。
>
> **正典の分担**:
> - **再生ロジック（頭脳）の契約** は SDK 側 `libs/ssruntime/docs/`（エンジン非依存の移植ガイド）が正典。本書は重複させず、必要箇所を**参照リンク**する。
> - **wgpu 固有の描画・FFI・配布** は本書が正典。SDK docs には混ぜない（あちらは SDK 内部情報を排除した Player 移植ガイドのため）。
> - 各アダプタ（TyranoScript 等の薄いグルー層）を将来足す場合は、本書への差分だけを書く（WebGL 版 `SSPLAYER_WEBGL_SPEC.md` + `TYRANO_COLDSTART.md` と同じ「正典1つ＋アダプタ差分」構造）。

---

## この一枚（cold-start 要約）

- **何を作るか**: SpriteStudio 7 の wgpu ベース Player。**第一級は Rust API**、その上に WebGPU(JS) と C/C++(FFI) を薄く被せる。
- **3層**: `ssruntime`(計算・既存) / `SSInternalPlayer`(描画) / `SSPlayer`(操作)。
- **幹の設計＝借用モデル**: wgpu の `Device`/`Queue`/`Surface` は**ホスト**が所有。`SSPlayer` は借りて与えられたターゲットへ描くだけ → Bevy 等へ**ロックインなしで相乗り**。
- **提供3面（ssruntime と同型）**: `rs`(rlib) / `js`(wasm-bindgen・`<ss-player>`互換) / `ffi`(cbindgen・DX12/Vulkan/Metal)。
- **データ所有契約**: `.ssab` は Player 所有→ssruntime へ borrow ／ FrameData・SoA は ssruntime 所有→Player が borrow（§1.1）。
- **ssab は embedded / reference 両対応**（テクスチャ埋め込み／外部参照）。
- **進捗**: **P0**（3層リファクタ＋借用モデル）✅ ／ **P4**（インスタンス §7.1・エフェクト §7.2）✅ 実機確認済。対応バッチ = Normal / Mesh / Instance / Effect。次は P2（色パリティ）/ P3（マスク）。詳細は §12。

---

## §0 目的と設計判断

### 0.1 目的（優先度つき）

| 優先 | 目的 | 成果物 |
|---|---|---|
| **最優先（土台）** | **Rust ネイティブ Player**（イディオマティックな Rust API） | 単一クレート `ssplayer-wgpu`（`internal`/`player` モジュール）。**任意の wgpu ホスト（自作アプリ / egui / macroquad / Bevy …）に相乗りできるエンジン非依存の土台**。以下2つはすべてこの Rust API の上に載る |
| **主（配布）** | **WebGPU 対応**（ブラウザ） | Rust API に wasm-bindgen を被せた単一 Rust→WASM バイナリ ＋ 既存 `<ss-player>` 互換 Web Component |
| **副（配布）** | デスクトップ各グラフィックス（DX12 / Vulkan / Metal）対応 | Rust API に cbindgen を被せたヘッダ付きライブラリ（cdylib / staticlib） |

**設計の順序**: まず**綺麗な Rust API を確定**し、WebGPU（wasm-bindgen）と FFI（cbindgen）はその薄いラッパとして設計する。Rust API が全インターフェースの根。

wgpu が単一コードベースから WebGPU / Vulkan / DX12 / Metal / GL を吸収するため、**描画コードは1つ**で全ターゲットに展開できる。これが本プレイヤーを wgpu で作る根拠。Rust API を wgpu ネイティブ（借用モデル）に設計すれば、wgpu を使う任意のホスト——自作アプリ・egui・macroquad・Bevy 等——へ**特定エンジンにロックインせず**相乗りできる。Bevy はその代表例にすぎない。

### 0.2 決定表（論点 / 決定 / 理由）

| 論点 | 決定 | 理由 |
|---|---|---|
| 立ち位置（最優先） | **Rust 用 Player が第一級**。他はその上に載せる | Bevy 等 Rust エコシステムへ展開する土台。綺麗な Rust API を先に確定してから枝を生やす |
| 三面提供モデル | **ssruntime と同じ rs / ffi / js を1クレートに集約**（`crate-type=["rlib","cdylib","staticlib"]` ＋ `ffi`/`web` feature）。層は `internal`/`player` モジュール | ssruntime 自身が単一クレート・多 crate-type の実証。別クレート分割は便益なくメンテコスト増 |
| クレート分割 | **単一クレート `ssplayer-wgpu`＋モジュール**。別クレートは viewer(bin) と将来の Bevy アダプタのみ | 別クレートが正当化される条件（別 semver publish / 独自リリース周期 / ssruntime 抜き再利用）に非該当。Bevy だけ独自リリース周期のため将来分離 |
| アーキテクチャ層 | **3層**（ssruntime / SSInternalPlayer / SSPlayer）をモジュールで表現 | 頭脳（計算）と体（描画）を分け、体をさらに「描画」と「操作」に分離。層境界はモジュール privacy で強制 |
| GPU デバイスの所有 | **SSPlayer は Device/Queue/Surface を所有しない（借用モデル）**。`&Device`/`&Queue` を借り、与えられた描画ターゲットへ描く。SSInternalPlayer も同様にターゲット非依存 | **エンジン非依存の相乗り**を可能にする。自作 wgpu アプリ / egui / macroquad / Bevy いずれも同じ API で組み込める。これは特定エンジンのためではなく「正しい Rust wgpu プレイヤーの形」 |
| ホスト薄ラッパ（デバイス所有） | **viewer / web / ffi が各々 wgpu 一式を生成・所有**し、借用モデルの SSPlayer を駆動 | 標準 Rust アプリ・ブラウザ・C ホストは自前でウィンドウ/canvas を持つ。所有はそこに閉じる |
| Bevy（ロックインしない） | **コア（internal/player）に Bevy 依存を入れない**。Bevy 統合は任意・別クレート `ssplayer-wgpu-bevy`（既定でワークスペース無効） | 借用モデルが既にエンジン非依存＝Bevy は"代表的なホストの一例"。別クレート化でロックインを構造的に防ぐ |
| ssruntime の取り込み方 | **Rust クレートとして直接リンク**（C-API 経由ではない） | WebGL 版の「WASM＋別JSレンダラ」2部構成と違い、全 Rust の単一バイナリ。配布もデバッグも素直 |
| 描画モード | **`RenderMode::Direct`（既定）** | Player 推奨。FrameData の頂点シリアライズをスキップし SoA バッファ経由で取得（ゼロコピー寄り） |
| JS 露出 API | **既存 `<ss-player>`（WebGL 版）と同一インターフェース** | 利用側資産をそのまま流用、WebGL→WebGPU をドロップイン差し替え可能に |
| コールドスタート doc | **単一正典 `SSPLAYER_WGPU_SPEC.md`（本書、リポジトリ内 `ssplayer-wgpu/` 直下）** | WebGL 版 `SSPLAYER_WEBGL_SPEC.md` と対称。分かりやすい |
| 最初のマイルストーン | **3層リファクタ優先** | 現プロトの `Player`/`Renderer` を internal/player/ffi へ再編し土台を固めてから機能追加 |
| 検証環境 | **Rust ネイティブ viewer 1本 ＋ C++ DX12 確認 viewer ＋ C++ Metal 確認 viewer ＋ Web 例** | ネイティブ C-API を per-backend 実機で検証するハーネス |
| ssab テクスチャ形態 | **reference / embedded の両対応**（`SSPlayerConfig` で経路を選ばず両方受ける） | ssconverter は「外部参照ssab」も「テクスチャ埋め込みssab」も出す。Player は両方をロードできる必要がある |
| プリマルチプライドアルファ | **全経路で premul 前提を貫く**（アップロード・シェーダ・ブレンド係数の三位一体） | どこか1つでも崩すと合成が破綻する。WebGL 版と同じ規律 |

---

## §1 アーキテクチャ（3層）

```
.sspj（XML）──ssconverter──▶ .ssab（FlatBuffers・変化点圧縮・コピーなし）
                                  │
   ┌──────────────────────────────┼──────────────────────────────────────┐
   │ 層1【計算 / 頭脳】 ssruntime（Rust, no_std・既存・改変しない）           │
   │   setup → update(delta) → get_frame_data → FrameData ＋ DrawBatch      │
   │   ＋ SoA バッファ（world_matrices / local_vertices / local_uvs / mesh_*）│
   └──────────────────────────────┼──────────────────────────────────────┘
                                  │  （Rust API を直接リンク）
   ┌──────────────────────────────┼──────────────────────────────────────┐
   │ 層2【描画 / 体】 SSInternalPlayer（Rust + wgpu, std）                    │
   │   FrameData/DrawBatch/SoA → wgpu の頂点/インデックス/描画コマンド        │
   │   パイプライン・シェーダ・ブレンド・テクスチャ・マスク(stencil)・エフェクト  │
   │   ★描画ターゲット非依存（与えられた RenderPass / TextureView へ描くだけ）  │
   └──────────────────────────────┼──────────────────────────────────────┘
                                  │
   ┌──────────────────────────────┼──────────────────────────────────────┐
   │ 層3【操作 / Rust API・第一級】 SSPlayer（Rust, std）                     │
   │   ssruntime Context 1個 ＋ SSInternalPlayer 1個 ＋ GPU リソース          │
   │   （pipeline/texture/buffer）を所有。★Device/Queue/Surface は所有しない  │
   │   new(&device,&queue,config) / update(&device,&queue,dt) / draw(&mut pass) │
   │   load / play / pause / stop / seek / speed / アニメ列挙 / 将来 override   │
   └──────────────────────────────┼──────────────────────────────────────┘
              借用（&Device, &Queue, &target）で駆動 ↑  ← ホストが所有
   ┌──────────────────────────────┴──────────────────────────────────────┐
   │ ホスト薄ラッパ（wgpu Instance/Device/Queue/Surface を所有）             │
   │   ① rs  : viewer（winit）/ Bevy プラグイン（Bevy の RenderDevice を渡す）│
   │   ② js  : web feature（wasm-bindgen, canvas→surface, <ss-player>互換）   │
   │   ③ ffi : cbindgen C-API（raw-window-handle→surface, DX12/Vulkan/Metal） │
   └───────────────────────────────────────────────────────────────────────┘
```

- **1 SSPlayer インスタンス ＝ 1 ssruntime Context**（REDESIGN_ASSESSMENT §4）。
- ssruntime は**ローカル座標**で「初期値＋変化点をマージ済み」の値を渡す。反転補正・座標系(Y-Up/Down)変換は**ssruntime 側で完結**しており SSInternalPlayer は不要（`local_vertices` にすべて反映済み）。
- **借用モデルが幹**: SSPlayer と SSInternalPlayer は `&Device`/`&Queue` を借り、与えられた描画ターゲット（`&TextureView` / `&mut RenderPass`）へ描くだけ。`Device`/`Queue`/`Surface` の所有は**ホスト**（viewer / Bevy / web / ffi）に閉じる。これにより Bevy のように自前の wgpu コンテキストを持つエンジンへ**そのまま相乗り**できる。
- GPU リソース（pipeline/texture/buffer）は借用した `&Device` から生成して SSPlayer が保持する。パイプラインは**描画ターゲットのフォーマット依存**なので、`new` 時にホストから target format を受け取る。

### 1.1 データ所有・ライフタイム契約（★厳守）

所有権の向きが**層で逆になる**。ここを取り違えると use-after-free になる。

| データ | 所有者（責務を持つ） | 参照側 | 渡し方 |
|---|---|---|---|
| **`.ssab`（アニメリソース）** | **Player（SSPlayer）** | ssruntime | Player が `SsabData`/`SsabResource` を保持し、ssruntime へは **borrow で渡す**（`bind_resource`）。ssruntime はこのリソースを所有しない |
| **FrameData / SoA バッファ** | **ssruntime** | Player | ssruntime が内部バッファに所有。Player は `get_frame_data` の戻り（`&[u8]` / SoA ポインタ）を **borrow として参照するだけ**。コピー・所有しない |

帰結（実装規約）:

- **ssab は Player より長生き**でなければならない。`SsabResource` を `Context`（ssruntime）より先に破棄しない。プロトは `Context<'static>` ＋ Player 保持の `Arc<SsabResource>` でこれを担保している（`ssplayer-wgpu/core/src/player.rs`）。
- **FrameData / SoA の借用は次フレームまで**。`get_frame_data` を呼ぶたびに ssruntime 内部バッファが書き換わるので、Player は**そのフレーム内で使い切る**（保持しない）。マルチスレッド描画では SoA ポインタの use-after-free に注意し、描画ジョブ完了を待ってから `update`/`destroy`。
- この契約は REDESIGN_ASSESSMENT §4 の「FlatBuffers をポインタ渡し／コピーなし」と一致。SSInternalPlayer は借用した FrameData/SoA を wgpu 頂点バッファへ**書き出した時点で借用を手放す**（GPU バッファ側が実体を持つ）。

---

## §2 最終プロジェクト構成

**単一クレート `ssplayer-wgpu`（`core/`）に3層をモジュールで集約する。**（✅=P0 実装済 / ◻=将来）

```
ssplayer-wgpu/
├── SSPLAYER_WGPU_SPEC.md          ← 本書（正典）
│
├── core/                          ← ★単一ライブラリクレート: ssplayer-wgpu（rs/js/ffi を集約）
│   ├── Cargo.toml                 [lib] crate-type = ["rlib"(+"cdylib","staticlib" when ffi)]
│   │                              features: web(wasm-bindgen) / ffi(cbindgen)。Device は所有しない
│   └── src/
│       ├── lib.rs                 ✅ 層の公開（SSPlayer が主 API）
│       ├── frame.rs               ✅ FrameView … ランタイム所有データへの借用（§1.1）
│       ├── internal.rs            ✅ 層2 SSInternalPlayer（描画。Context/Device 非所有）
│       │                             ◻ 分割時: pipelines / mask / effect / texture サブモジュール
│       ├── player.rs              ✅ 層3 SSPlayer（操作。Context+SSInternalPlayer 所有）
│       │                             ◻ インスタンス子ごと独立 Context の再生木（§7）
│       ├── web.rs                 ◻ #[cfg(feature="web")] wasm-bindgen + <ss-player>互換（② js）
│       └── ffi.rs                 ◻ #[cfg(feature="ffi")] extern "C" ss_player_*（③ C/C++）
│                                     build.rs で cbindgen → include/ssplayer_wgpu.h
│
├── viewer/                        ← ✅ 検証: Rust ネイティブ viewer（bin, ① rs のホスト例）
│   └── src/main.rs                winit + wgpu 一式を所有し、借用モデルの SSPlayer を駆動
│
├── bevy/                          ← ◻【将来・別クレート】Bevy アダプタ（借用モデルの一利用例）
│                                     crate: ssplayer-wgpu-bevy。★コアはこれに依存しない
│                                     独自リリース周期のため、これだけは別クレート化
│
└── examples/                      ← ◻ マルチプラットフォーム動作確認環境
    ├── web/                       WebGPU ブラウザ確認（<ss-player-webgpu>）
    ├── viewer-dx12-cpp/           C++ DX12 確認 viewer（Win32 HWND → C-API → wgpu が DX12 強制）
    └── viewer-metal-cpp/          C++ Metal 確認 viewer（Cocoa CAMetalLayer → C-API → Metal 強制）
```

> js（`web`）は feature、ffi は feature ＋ crate-type 追加で**同一クレート内**に載る（ssruntime と同型）。別クレートは viewer(bin) と将来の Bevy のみ。Vulkan 確認 viewer（Linux/C++）も `examples/viewer-vulkan-cpp/` として同型で追加可。

### 2.1 Cargo ワークスペース登録

ルート `Cargo.toml` の `members` は現状のまま（`ssplayer-wgpu/core`・`ssplayer-wgpu/viewer`）。将来 Bevy を足す時だけ `ssplayer-wgpu/bevy` を追加。C++ の examples は Cargo ワークスペース外で、CMake が cbindgen 生成ヘッダ＋ビルド済みライブラリを消費する。

---

## §3 ssruntime 契約（参照）

> 詳細な正典は `libs/ssruntime/docs/30_core/30_render_loop.ja.md` と `libs/ssruntime/SCHEMA.ja.md`。ここでは wgpu 実装に必要な**接点だけ**要約する。

### 3.1 毎フレームの呼び出し順（厳守）

```
setup_animation(name)            ← 1回
loop {
  update(delta_ms) → frame_no    ← ssruntime Context::update（delta はミリ秒）
  get_frame_data(frame_no) → &[u8]   ← ★これが内部 SoA バッファを書き込む
  // 以降、SoA getter は get_frame_data 後の内部バッファを指す。
  // 次フレームの get_frame_data で無効化され得る（use-after-free 注意）
  root_as_frame_data(bytes)      ← FlatBuffers 参照
  for batch in draw_batches { ... 描画 ... }
}
```

### 3.2 SoA アクセサ（`Context::get_*_ptr` / `_len`、C-API は `ss_runtime_get_*`）

`_len` は**要素数（float/整数の総数）**で返る点に注意。

| アクセサ | 型 | レイアウト | 意味 |
|---|---|---|---|
| `world_matrices` | `*const f32` | `part_index*16`、**列優先** 4x4 | 各パーツの最終ワールド行列。`wx = m[0]*lx + m[4]*ly + m[12]` |
| `local_vertices` | `*const f32` | `part_index*10` = `[lt,rt,lb,rb,center]`×xy | pivot/flip/デフォーム/座標系反転すべて適用済み。ワールド行列を掛けるだけ |
| `local_uvs` | `*const f32` | `part_index*10` | UVアニメ/反転適用済み。**テクスチャ pixel 空間**なのでサイズ除算で正規化 |
| `cell_meta` | `*const f32` | `part_index*6` = `[pivot,size,rect_lt]` | セル矩形（source rect 構築用。任意） |
| `mesh_vertices_x` / `_y` | `*const f32` | フラット、offsets で切る | スキニング済みメッシュ頂点（**ワールド空間済み・行列不要**）。X/Y 別 SoA |
| `mesh_vertex_offsets` | `*const u32` | parts+1 prefix-sum | `[offsets[i], offsets[i+1])` |
| `mesh_uvs` | `*const f32` | 頂点×2 | メッシュ UV（pixel 空間、正規化必要） |
| `mesh_indices` | `*const i32` | パーツ相対 | `base_vert_idx + rel_idx` で再ベース |
| `mesh_index_offsets` | `*const u32` | parts+1 prefix-sum | インデックス範囲 |
| `shape_vertices` / `_vertex_box_coords` / `_vertex_counts` | — | Shape パーツ用 | 現プロト未使用。Shape 実装時に使う |

FlatBuffers 側から読む: `draw_order`（`[ushort]` parts インデックス）/ `draw_batches` / `parts`(`PartState`) / `parts_color`（頂点カラー）。

> **性能注意**: `parts_color` の per-element アクセサは高コスト（多体で顕著）。多数体では生読みミラー化を検討（`render_loop.ja.md`）。

### 3.3 SSInternalPlayer が担う責務（＝ ssruntime 対象外）

- **マスクのラスタライズ**（ステンシル／クリッピング実装）。ssruntime は `DrawBatchKind::Mask` 発行・`mask_indices`・`flags` 提供のみ。
- **テキストのグリフレンダリング**（レイアウト情報のみ提供）。
- **エフェクト／インスタンスの実描画**（Slot API で step して得た `EffectDrawPlan` を描く）。
- **AABB 当たり判定**（`bounding_radius` 補間のみ提供）。
- 反転補正・座標系変換は**不要**（ssruntime 側で完結）。

---

## §4 描画契約（DrawBatch）

### 4.1 DrawBatch（`framedata.fbs`、inline struct）

`start_rank`(ushort) / `count`(ushort) / `texture_hash`(uint32) / `vertex_count`(ushort) / `index_count`(ushort) / `blend_type` / `kind` / `flags`(ubyte)。

- `draw_order[start_rank .. start_rank+count]` が覆うパーツ列（値は `FrameData.parts` のインデックス）。
- `texture_hash`: バインドするテクスチャの fnv1a ハッシュ。不要種別（Shape 等）は `0`。
- `vertex_count`/`index_count`: バッファ事前確保用のバッチ内合計。
- `flags`: `0x01 = MASK_INFLUENCE`（このスライスがアクティブマスクの影響下）。未知ビットは無視。

### 4.2 DrawBatchKind（全8種）と実装状況

| kind | ジオメトリソース | 実装 | 優先 |
|---|---|---|---|
| `Normal` | vertex + cell（標準スプライト矩形） | ✅ 実装済 | — |
| `Mesh` | スキニング済みメッシュ | ✅ 実装済 | — |
| `Instance` | サブアニメのインスタンススロット | ✅ 実装済（§7.1） | — |
| `Effect` | パーティクル矩形（`EffectDrawPlan`） | ✅ 実装済（§7.2） | — |
| `Shape` | 手続き的シェイプ（テクスチャ無し、shape_* SoA） | ✅ 実装済（三角形ファン＋PartColor bilinear、白フォールバック） | — |
| `Mask` | ステンシル書き込み専用（色出力なし） | ✅ 実装済（§6） | — |
| `Text` | ビットマップフォントグリフ | ❌ | 低（旧版も未実装） |
| `Nines` | 9パッチ | ❌ | 低（旧版も未実装） |

> "External" は独立 kind ではない。外部インスタンスは `Instance` kind ＋ Slot API（別 ssab のロードが Player 側で必要）。

### 4.3 BlendType（全12値）

`Mix, Mul, Add, Sub, Mulalpha, Screen, Exclusion, Invert, Mul2, Div2, Screen2, Overlay2`。
現プロトは `Mix/Add/Sub/Mul` の4種のみ（他は Mix フォールバック）。§5 のパイプラインマトリクスで全種を事前生成する。

---

## §5 SSInternalPlayer 描画設計

### 5.1 頂点フォーマット（WebGL 版と同一 SoA 意味論）

| 属性 | loc | 型 | 内容 |
|---|---|---|---|
| position | 0 | vec2/vec3 | ワールド座標（Normal は世界行列適用後、Mesh は済み＋z） |
| uv | 1 | vec2 | 正規化 UV（0..1） |
| color | 2 | vec4 | RGBA 0..1（パーツカラー頂点色 or α） |
| varg | 3 | vec4 | `[rate, isMix, isMul, isAddSub]`（PartsColor ブレンド制御） |

> 現プロトの `Vertex{position:[f32;3], uv, color}` に **`varg: [f32;4]` を追加**してカラーブレンド率に対応させる（WebGL 版と揃える）。

Normal パーツは builder が index を出さないので、5頂点（TL,TR,BL,BR,center）に対し中心を要にした扇 12 index を張る：`(0,1,4)(1,3,4)(3,2,4)(2,0,4)`。現プロトは4隅2三角形だが、頂点変形/カラーで center が効く場合に扇へ切替。

### 5.2 パイプライン設計（WebGL の動的 state → wgpu は焼き込み）

**wgpu では BlendState / DepthStencilState は RenderPipeline に焼き込む**。よって WebGL の `applyBlend`（動的 `blendFuncSeparate`）は消え、代わりに **組合せ分のパイプラインを事前生成して差し替える**。

```
パイプライン ≈ BlendType(12) × 用途{parts, effect, mask-write, mask-test, ss-shader×13}
```

現実には必要な組合せだけ遅延生成 or 起動時生成。去重は「同一パイプラインなら再バインドしない」（現プロトの `_lastBlend` 相当）。

| BlendType | color src, dst | operation |
|---|---|---|
| Mix | `One, OneMinusSrcAlpha` | Add |
| Add | `One, One`（premul） | Add |
| Mul | `Dst, OneMinusSrcAlpha` | Add |
| Sub | `One, One` | **ReverseSubtract**（RGBのみ。alpha は Add） |
| Screen | `OneMinusDst, One` | Add |
| Invert | `OneMinusDst, Zero` | Add |
| Exclusion | `OneMinusDst, OneMinusSrc` | Add |
| その他 | 上記に準拠（未検証分は Mix フォールバック→順次検証） | Add |

alpha は全モード `src=One, dst=OneMinusSrcAlpha`（premul 前提）。

### 5.3 カラーブレンド WGSL（WebGL の `PartsColorProgram` を直訳）

テクスチャは premul 済みなので**逆premul→カラー演算→再premul→階層α畳み込み**：

```wgsl
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    let straight = tex.rgb / max(tex.a, 0.0001);            // 逆premul
    let rate = in.varg.x;
    let mixC = mix(straight, in.color.rgb, rate);
    let mulC = straight * mix(vec3(1.0), in.color.rgb, rate);
    let addC = straight + in.color.rgb * rate * sign(in.varg.w);
    let rgb  = mixC*in.varg.y + mulC*in.varg.z + addC*abs(in.varg.w);
    let a    = tex.a * in.color.a;
    return vec4(rgb * a, a);                                 // 再premul（階層αは uniform で別途）
}
```

### 5.4 投影・座標系

正射影（原点中央・Y-down）＋ 配置アフィン。現プロトの `update_projection`（mat4）を踏襲。`get_coordinate_system` で問い合わせ可能だが頂点変換は不要（ssruntime 側で反映済み）。

---

## §6 マスク（ステンシル）✅ 実装済（`core/src/mask.rs` ＋ `internal.rs`/`player.rs`）

- **`MaskPlanner`（WebGL 版 core・エンジン非依存）の rank 昇順スコープ replay ロジックを Rust に移植**（`core/src/mask.rs`：`plan_writers`/`MaskWriter`/`PartMaskFlags`/`StencilTest`/`MaskOp`）。ライタごとに rank / OP(INCR|INVERT) / cutout閾値 / clipping フラグを持つ。**ライタの texture は所属 DrawBatch から取る**（pure Mask batch は `tex=0`→閾値 -1＝カットアウト無しで全ジオメトリ、clipping は content の実 texture）。
- **wgpu では stencil op もパイプラインに焼く**：write 用（IncrementClamp/DecrementClamp/Invert）と test 用（NotEqual/Equal, `stencil write_mask=0`）で別パイプライン。ステンシル ref は常に 0（ops/compare をパイプラインに焼くため動的変更不要）。content パイプラインは `blend×stencil_test(None/Inside/Outside)` のマトリクス、カットアウト閾値は mask-write 頂点の `color[0]` に載せ `fs_mask` が discard。
- 手順（`40_components/40_mask.ja.md` 準拠）:
  1. pure マスクを事前書き込み（コンテンツ描画前に全ステンシル書き込み、`ColorWrites::empty()`）。
  2. コンテンツを rank 昇順で描画しつつ、`sync_masks_to(rank)` で clipping ライタ追加 / pure ライタ除去。
  3. マスク対象は polarity 判定（`visible_inside_mask` → `NotEqual,0`(内側) / `Equal,0`(外側)）。**polarity がバッチ内で変わるためパーツ毎描画（バッチ化しない）**。
  4. **pure マスク（`PartTypeMask`/`draw_as_mask`）は色を描かない**：runtime は pure マスクにも content batch を発行するので、`render_node` で `pure_mask()` パーツの描画をスキップ（§3-3）。
- **インスタンス継承**（§7.1 木の再帰に `inherited_stencil` を伝播）：被マスク対象がインスタンスなら子アニメ全体を親ステンシルでクリップ（screen-space・単段、`40_mask.ja.md §3-2-2`）。
- **サーフェス/描画ターゲットに stencil 付きデプスバッファを確保する**必要がある。`SSPlayerConfig.stencil_format: Option<TextureFormat>` で opt-in（viewer は `Depth24PlusStencil8` を確保し render pass に attach、`None` なら従来どおりマスク無効で回帰なし）。
- **白フォールバックテクスチャは必ず白で初期化**：1x1 テクスチャは `create_texture` だけでは透明黒（ゼロ初期化）で、Shape/テクスチャ欠落 content が `tex*color=0` で不可視になる。`new()` は借用モデルで queue を持たないため、`end_frame` 初回に `queue.write_texture([255;4])` で遅延初期化（`white_ready` フラグ）。
- 既知の限界: INCR と INVERT 混在の重なりマスクは単一ステンシル値で近似（厳密解は coverage-bitmap 方式）。子アニメ内部定義のマスクは非適用（WebGL 版と同一）。エフェクト被マスクは content パイプライン流用で対応済。
- 検証: `core/tests/mask.rs`（planner probe、Mask.ssab の writer/Shape-mask 分類）＋ `core/tests/render.rs`（ヘッドレス GPU offscreen readback：masking on/off の差分・Shape content の描画量で回帰検出）。

---

## §7 エフェクト / インスタンス（Slot API）

Godot / Unity のネイティブプレイヤー実績と C-API 契約から、この節の設計は確定している。

### 7.1 インスタンス＝「子ごと独立 Context」の再生木（★確定）

- **各インスタンス子アニメに独立した Context（＝独立した SoA バッファ）を割り当てる。** これが SoA 単一バッファ上書き問題の構造的解法で、`ss_instance_slot_step(slot, ev, child_ctx, ...)` が **child_ctx を引数で要求する** C-API 契約そのもの。子の `get_frame_data` は子 Context の SoA に書くので親を汚さず、**再取得もコピーも不要**。
- **再帰の単位は「再生ノード」＝ Context ＋ 各種 Slot ＋ 子ノード群**。SSPlayer はこれを木として所有する（親→子→孫）。InstanceSlot / EffectSlot は各ノードの再生層が part_index で保持。子 Context が独立なので**全ノードの FrameView が同時に有効**であり、順に SSInternalPlayer へ渡せる。
- **SSInternalPlayer は木に含めない（案A）。** GPU リソース（pipeline/texture/累積頂点バッファ）は木全体で1個を共有所有。テクスチャは hash がグローバルなので子アトラスも共有マップに載る。描画層は Context を一切持たず、渡された FrameView 群を消費するだけ。
- **描画順**: 子は親の Instance パーツの rank 位置に一塊として差し込む（親パーツ列の途中に子が入る）。
- **wgpu の実装型は Unity 型**: 子はローカル空間で頂点/メッシュを中間バッファに事前ビルド → 親がその頂点を world 行列で変換して累積バッファへ merge（子の完了を依存に持つ）。Godot 型（子が自分の CanvasItem に先に描き親が RID 再ペアレント）は wgpu に retained scene graph が無いため移植不可。Context 独立を活かした `get_frame_data` の並列化も可能。
- ExternalInstance は参照先 ssab を SSPlayer が別途ロードし、その Context を子ノードとして木に接ぐ。

**実装メモ（✅ 実装済・`core/src/player.rs`）**

- `PlaybackNode { context, instances, effects }` の木を SSPlayer が所有。GPU リソースは `SSInternalPlayer` 1個を共有。
- **外部参照は pack 名の文字列**（`PartTypeInstance::ref_anime_pack()`/`ref_anime_name()`）で解決＝ハッシュアルゴリズム不整合を回避。ホストは全 pack を `add_ssab(pack_name, data)` で事前登録し、`load_ssab(pack_name, data)` で主 pack 指定。ssab 型は `ssab::ssab::ss::format::{PartType, root_as_ss_anime_binary_unchecked}`。
- **transition スナップはホスト責務**: `InstanceSlot::step` は transition 時にセクション/ループ/方向を設定するのみで開始フレームへのスナップはしない。**`result.transitioned` 時に子 `context.play()` を呼ぶ**（呼ばないと独立モードが再生状態にならず進まない）。
- **`parent_looped` はランタイム非通知**: root は前フレームからのフレーム後退で検出（前向き再生のループ edge）、子は `result.child_looped` を伝播。
- 検証: `core/tests/instance.rs`（ヘッドレス probe。同期/独立/pingpong の frame 推移を確認）。

### 7.2 エフェクト（別 Context を持たない）✅ 実装済

- エフェクトは**別 Context 不要**（親 Context の frame_no/delta で駆動するシミュレータのため上書き問題が無い）。`EffectSlot<'static>` を再生ノードが part_index で保持。`EffectSlot::new(resource, PartTypeEffect::ref_effect_name_hash(), seed=part_index)`。
- `slot.step(ev, frame_no, delta_secs, fps, parent_looped, y_flip=false, vert_stride=2)` → `ss_effect_step_result{ visible, draw_plan_buf: *const u8, draw_plan_len }`。`draw_plan_buf` はスロット所有・次 step まで有効なので**即コピー消費**。`ev = ctx.get_active_effect_event(part_index)`、`fps = ctx.get_fps()`。
- `EffectDrawPlan`（`root_as_effect_draw_plan_unchecked`）: `commands`（`cellmap_hash`/`quad_count`/`particle_offset`/`blend` 0=Mix 1=Add）、`vert_stride`、`verts`/`uvs`/`colors`/`indices`。スライス式（`po=particle_offset`）: verts `[po*4*stride..(po+qc)*4*stride]` / uvs `[po*4*2..]` / colors `[po*4*4..]` / indices `[po*6..]`（**quad-local 0基点、rebase 不要**）。
- **★エフェクト UV は正規化済み(0..1)**: スプライト/メッシュの pixel UV と違い、**tex_size で割ってはいけない**（probe で確認・当初バグを修正）。colors も正規化。
- verts は **slot-relative** → エフェクトパーツの world 行列を transform 合成（`mat4_mul(transform, effectPartWorld)`）。
- **1 command = 1 draw call**（`cmd.blend` でパイプライン切替、`cmd.cellmap_hash` でテクスチャ）。エフェクトは PartsColor を持たず、plan の色が既存 tex*color シェーダを変調するので varg 不要。実装: `core/src/internal.rs::append_effect`。

---

## §8 テクスチャ管理

- **hash → `{texture, width, height}`** の Map（`core/src/internal.rs`）。textureHash は atlas 識別子（fnv1a）。
- **premultiply を必ず適用**（SS 必須）。ネイティブは RGBA デコード後に自前で premul、または premul 済みテクスチャを供給。ブラウザは `UNPACK_PREMULTIPLY_ALPHA` 相当をアップロード経路で担保。
- フィルタ/wrap: `ClampToEdge`（NPOT 対応）＋ `Linear`、mipmap なし。
- **texture_hash 0 / 未ロード → 1x1 白テクスチャ**にフォールバック（Shape content・純カラー対応）。**この白テクスチャは必ず白(255,255,255,255)で初期化すること**（`create_texture` だけだと透明黒で `tex*color=0` になり Shape が不可視）。`new()` は queue を持たないため `end_frame` 初回に `write_texture` で遅延初期化（§6）。

### 8.1 ssab の2形態に両対応（embedded / reference）

ssconverter は `.ssab` を2形態で出力しうるので、SSPlayer はどちらでもロードできる必要がある。

| 形態 | 内容 | Player の扱い |
|---|---|---|
| **embedded** | テクスチャ（atlas PNG）を `.ssab` に埋め込み | `load_ssab` 時に **Player が埋め込みバイト列を列挙 → デコード → premul → upload_texture** まで自動で行う（自己完結、単一ファイル配布に最適・広告向け） |
| **reference** | テクスチャは外部ファイルを名前/hash で参照 | Player は hash だけ把握。**実バイトはホストが供給**（`upload_texture(hash, rgba, w, h)`）。ロード経路（ファイル/URL/バンドル）はホストの責務 |

- **統一供給 API**: 最終的にはどちらも `upload_texture(hash, premul済みRGBA, w, h)` に集約される。embedded は Player 内部でこの呼び出しを行い、reference はホストが行う。ブラウザの `TexImageSource` 供給・ネイティブの RGBA 供給と対称。
- `SSPlayerConfig` で「embedded を自動ロードするか」「PNG デコーダをどこに置くか（Player 内蔵 `image` クレート or ホスト供給コールバック）」を選べるようにする。WASM サイズを抑えたいブラウザでは、デコードをブラウザ（`createImageBitmap`）に委ねる選択肢も残す。
- **要調査**: 埋め込みテクスチャの列挙 API（ssab/ssruntime 側で cellmap エントリ → 埋め込みバイト列を引く経路）の正確な形。`libs/ssab/SCHEMA.md` と `SsabResource` を確認。

---

## §9 Rust API（第一級・全インターフェースの根）

**設計原則**: SSPlayer は wgpu をイディオマティックに借用する Rust API。`Device`/`Queue`/`Surface` を一切所有せず、ホスト（viewer / Bevy / web / ffi）が所有する wgpu コンテキストを借りて描く。これにより Bevy の render graph へそのまま組み込める。

### 9.1 SSInternalPlayer（層2・描画）✅ 実装済（`core/src/internal.rs`）

累積 API: ホスト（SSPlayer）がバッチ走査を駆動し、ノード/バッチ単位で `append_*` を呼ぶ（インスタンスの rank 差し込み・transform 合成のため）。

```rust
pub struct SSInternalPlayer { /* pipelines, textures, buffers, vertices/indices, draw_calls */ }

impl SSInternalPlayer {
    pub fn new(device: &wgpu::Device, config: &RendererConfig) -> Self;
    pub fn update_projection(&self, queue: &wgpu::Queue, w: f32, h: f32, y_down: bool);
    pub fn upload_texture(&mut self, device, queue, hash: u32, rgba: &[u8], w: u32, h: u32);

    // --- 1 フレームの累積（Unity 型フラット化）---
    pub fn begin_frame(&mut self);                                   // クリア
    pub fn append_batch(&mut self, frame: &FrameView, transform: &[f32;16], batch: &BatchInfo);   // Normal/Mesh
    pub fn append_effect(&mut self, plan_buf: &[u8], transform: &[f32;16]);                         // EffectDrawPlan
    pub fn end_frame(&mut self, device, queue);                     // GPU へアップロード

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) -> usize;
}
```

`RendererConfig { color_format }` はホストの描画先仕様（マスク実装時に depth_stencil_format 追加）。`FrameView` は ssruntime の FrameData ＋ SoA スライスをまとめた借用ビュー（`get_frame_data` 直後に構築）。`BatchInfo { start_rank, count, texture_hash, blend_type, kind }` は SSPlayer が DrawBatch から抽出して渡す。`transform` は root=単位、子=親 world 行列合成。

### 9.2 SSPlayer（層3・操作）— 幹の公開 API ✅ 実装済（`core/src/player.rs`）

```rust
pub struct SSPlayer { /* SSInternalPlayer, resources: Map<pack, Arc<SsabResource>>, root: PlaybackNode 木, ... */ }

impl SSPlayer {
    pub fn new(device: &wgpu::Device, config: &SSPlayerConfig) -> Self;

    // --- pack 登録 / 再生制御 ---
    pub fn add_ssab(&mut self, pack_name: &str, data: &[u8]) -> Result<(), SSRuntimeError>;   // 外部参照解決用に全 pack 登録
    pub fn load_ssab(&mut self, pack_name: &str, data: &[u8]) -> Result<(), SSRuntimeError>;  // 主 pack 指定
    pub fn play(&mut self, anim: &str) -> Result<(), SSRuntimeError>;   // インスタンス木を再構築
    pub fn pause(&mut self);  pub fn stop(&mut self);  pub fn set_speed(&mut self, speed: f32);
    pub fn is_playing(&self) -> bool;
    pub fn animation_names(&self) -> Option<Vec<String>>;
    pub fn canvas_size(&self) -> Option<(f32, f32)>;

    // --- テクスチャ供給（reference 用。最終的に premul 済み RGBA へ集約）---
    pub fn upload_texture(&mut self, device, queue, hash: u32, rgba: &[u8], w: u32, h: u32);
    pub fn set_viewport(&self, queue, w: f32, h: f32, y_down: bool);

    // --- 毎フレーム ---
    pub fn update(&mut self, device, queue, delta_secs: f32);   // 木を進め、begin→append→end で累積
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) -> usize;
}
```

**借用モデルの要点**: `update`/`draw` は毎回 `&Device`/`&Queue` を受け取る。`draw` はホストが用意した `RenderPass` に積むだけで、`begin_render_pass`/`submit`/`present` はホストの責務。

**外部インスタンス解決**: `add_ssab(pack_name, data)` で全 pack を pack 名キーで登録 → インスタンスは `ref_anime_pack()` 文字列で resolve（§7.1）。

> **現状と将来**: `new` は device のみ（`Result` 不要）、エラーは当面 `SSRuntimeError` を流用。将来 `SSPlayerError`（`thiserror`）でラップし FFI 整数コード / JS `JsError` へ変換。ssab の **embedded 自動テクスチャロード**（§8.1）と **seek** は未実装（reference-mode ＋ viewer での目視が現状）。

### 9.3 各ホストからの駆動（rs / js / ffi 共通の型）

- **① rs / viewer**: winit で Surface を所有 → 毎フレーム `get_current_texture` → `SSPlayer::update` → `begin_render_pass` → `SSPlayer::draw` → `submit`/`present`。
- **① rs / Bevy**: Bevy の `RenderDevice`/`RenderQueue`（内部は wgpu）を借りて `SSPlayer::update`/`draw`。描画先は Bevy の render target。
- **② js / web**: `web.rs` が canvas から Surface/Device を生成・所有し、上と同じループを回す。JS へは `<ss-player>` 互換で露出。
- **③ ffi / C・C++**: `host.rs` が raw-window-handle から Surface/Device を生成・所有し、`ss_player_*` ハンドル API 越しに同じループを回す。

### 9.4 将来 API（REDESIGN_ASSESSMENT §7）

パーツ単位オーバーライド（色/セル/表示）、ミキシング、ステートマシン。いずれも ssruntime 側の合成で完結し FrameData/DrawBatch 契約を壊さないため、SSPlayer に制御メソッドを足すだけで SSInternalPlayer の改修は不要。

---

## §10 WebGPU / JS 面（② js — Rust API の上のホスト）

- **単一 Rust→WASM バイナリ**（ssruntime + internal + player を wasm-bindgen で1つに）。wgpu が WebGPU バックエンドを叩く。
- `core/src/web.rs`（`#[cfg(feature="web")]`）が **canvas から Surface/Device を生成・所有**し、借用モデルの SSPlayer（§9）を駆動する薄いホスト。wasm-bindgen 束縛はこの層。
- **JS 露出は既存 `<ss-player>`（WebGL 版）と同一インターフェース**にし、WebGL→WebGPU をドロップイン差し替え可能に。
- ssab は embedded / reference 両対応（§8.1）。embedded なら単一ファイルで完結（広告向け）、reference ならテクスチャを JS 側から供給。
- ビルド/パッケージは `scripts/release-web.sh` 相当を追加し `pkg/ssplayer-wgpu-web/` を吐く。
- 注意: WebGPU 未対応ブラウザへのフォールバック方針（WebGL 版へ退避 or エラー表示）を Web Component 層で定義。

---

## §11 ネイティブ / C・C++ 面（③ ffi — Rust API の上のホスト）

- `core` の `ffi` feature（`crate-type` に cdylib / staticlib を追加）。`build.rs` で cbindgen → `include/ssplayer_wgpu.h`。
- `core/src/ffi.rs` が **raw-window-handle から Surface/Device を生成・所有**し、借用モデルの SSPlayer を駆動する薄いホスト。C からはハンドルベース API で操作:

```c
typedef void* ss_player_t;
ss_player_t ss_player_create(const ss_player_desc_t* desc);   // 生ウィンドウハンドル + backend + size
void        ss_player_destroy(ss_player_t);
int         ss_player_load_ssab(ss_player_t, const uint8_t* data, size_t len);  // embedded/reference 両対応
int         ss_player_play(ss_player_t, const char* anim_name);
void        ss_player_update(ss_player_t, float delta_seconds);
void        ss_player_add_texture(ss_player_t, uint32_t hash, const uint8_t* rgba, uint32_t w, uint32_t h);  // reference 用
void        ss_player_render(ss_player_t);                    // swapchain 取得 → 描画 → present
void        ss_player_resize(ss_player_t, uint32_t w, uint32_t h);
```

- **自己完結ホスト（既定）**: `ss_player_desc_t` に**生ウィンドウハンドル**（Windows: HWND+HINSTANCE / macOS: NSView or CAMetalLayer / Linux: Xlib/Wayland）＋ backend ヒント（DX12/Vulkan/Metal/自動）を渡し、`host.rs` が Surface/Device を生成。per-backend 確認 viewer が各バックエンドを強制指定して検証。
- **既存エンジンのデバイス相乗り（将来・借用モデルの延長）**: SSPlayer 自体が Device を借用する設計なので、ホストの `Device`/`Queue`/`RenderTarget` を wgpu-hal 相互運用で受け取る `ss_player_create_borrowed(...)` を後付けできる。これは Rust 側の Bevy 統合と同じ借用経路の C 版。
- パッケージは `release-macos.sh` / `release-windows.ps1` / `release-linux.sh` 相当で cdylib/staticlib + ヘッダを配布。

---

## §12 実装フェーズ（初手 = 3層リファクタ）

| Phase | 内容 | 完了条件 |
|---|---|---|
| **P0（初手）3層リファクタ＋借用モデル Rust API** ✅ | 現 `core`（Player/Renderer）を単一クレート内の `internal`(SSInternalPlayer) / `player`(SSPlayer) モジュールへ再編。**SSPlayer を借用モデル（`&Device`/`&Queue` を借りる §9）に確定**。データ所有契約（§1.1）を配線。viewer が wgpu 一式を所有し SSPlayer を駆動 | 現状の描画（Normal/Mesh/4blend）が3層＋借用モデルで従来通り動く。**これが ① rs 面の土台**（完了） |
| P1 ssab 両対応 + WebGPU（② js） | ssab の embedded/reference 両ロード（§8.1）。`core` に `web` feature（wasm-bindgen）。`examples/web` で `<ss-player>` 互換 Web Component、canvas 表示 | embedded ssab がブラウザで WebGPU 描画される |
| P2 描画パリティ① | varg 追加（カラーブレンド率）、BlendType 全12種のパイプラインマトリクス、premul 厳密化 | WebGL 版と色・ブレンドが一致 |
| **P3 マスク** ✅ | stencil 付きターゲット（config opt-in）、MaskPlanner 移植（`core/src/mask.rs`）、write/test パイプライン、pure マスク色スキップ、インスタンス継承、**Shape 幾何のマスク writer ＋ Shape content 描画**、白フォールバック初期化（§6） | マスク/Shape アニメが描画（Mask.ssab / Shape.ssab をヘッドレス GPU readback ＋ 実機で確認） |
| **P4 エフェクト/インスタンス** ✅ | **P0 の次に先行実施**。①内部を `begin_frame`/`append_batch`/`append_effect`/`end_frame` の累積 API＋transform へ（Stage 1）②インスタンス木＝子ごと独立 Context（Stage 2, §7.1）③エフェクト＝EffectSlot/EffectDrawPlan（Stage 3, §7.2） | エフェクト/インスタンスが描画される（実機確認済） |
| P5 ネイティブ C-API + C++ viewer（③ ffi） | `core` の `ffi` feature ＋ cbindgen、`examples/viewer-dx12-cpp`・`viewer-metal-cpp` | DX12/Metal 実機で同一ライブラリが描画 |
| P6 追加ブレンド / 統合シェーダ13種 | 残バッチ種別（Shape は P3 で対応済。BlendType 全12種・varg・ss_shader が残り） | フル機能パリティ |
| （任意）Bevy アダプタ | `ssplayer-wgpu-bevy`（借用モデルの一利用例）。コアに依存を入れず別立て | Bevy の render graph で SSPlayer が描画 |
| （後回し）Text / Nines | 旧版も未実装 | 必要時 |

---

## §13 検証

| 対象 | 手段 |
|---|---|
| Rust 描画（開発） | `viewer/`（winit+wgpu、`tests/overall/overall/` 再生。横=ssab切替/縦=アニメ切替） |
| インスタンス/エフェクト（ヘッドレス） | `core/tests/instance.rs`（probe。frame 推移・quad 生成を GUI 抜きで確認） |
| マスク/Shape（ヘッドレス） | `core/tests/mask.rs`（planner probe：writer 分類・Shape-mask）＋ `core/tests/render.rs`（GPU offscreen readback：masking on/off 差分・Shape content 描画量。adapter 無ければ skip。PNG 保存で目視も可） |
| WebGPU（主） | `examples/web/`（ブラウザで `<ss-player-webgpu>`）。初回起動計測（取得→wasm初期化→初フレーム ms） |
| DX12（副） | `examples/viewer-dx12-cpp/`（Windows 実機、backend=DX12 強制） |
| Metal（副） | `examples/viewer-metal-cpp/`（macOS 実機、backend=Metal 強制） |
| Vulkan（将来） | `examples/viewer-vulkan-cpp/`（Linux 実機、backend=Vulkan 強制） |
| WebGL 版との一致 | 同一 ssab を両者で再生し目視/スクリーンショット比較（色・ブレンド・マスク） |
| Rust 品質 | `cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --all -- --check` / `cargo test` |

---

## §14 既知の制約・要調査

- **premul の一貫性**が最重要。アップロード・シェーダ・ブレンド係数のどこか1つでも崩すと合成破綻。
- **stencil ターゲット**: マスク実装時にサーフェス構成へデプス/ステンシルを追加（現プロトは無し）。
- **INCR/INVERT 混在マスク**は単一ステンシルで近似（厳密解は coverage-bitmap、要検討）。
- **WebGPU 非対応環境のフォールバック**方針（WebGL 退避か）未定。
- **raw-window-handle の FFI 越え**: Rust トレイトは C に渡せないため、プラットフォーム別の生ハンドル構造体を C-API に定義して内部で組み立てる。
- **テクスチャ供給**: embedded（Player がデコード）/ reference（ホスト供給）の両対応（§8.1）。埋め込みテクスチャ列挙 API の正確な形は `libs/ssab/SCHEMA.md` / `SsabResource` で要確認。
- **データ所有契約（§1.1）**: ssab は Player 所有→ssruntime へ borrow、FrameData/SoA は ssruntime 所有→Player が borrow。ssab を Context より先に破棄しない／FrameData 借用をフレーム跨ぎで保持しない。
- **エフェクト blend はコマンド毎**（batch.blend_type ではない）点を実装で取り違えない。
- **ss_shader 統合シェーダ13種**の param マッピングは WebGL 版 `SSShaderLib.ts` を参照。

---

## §15 参照

**SDK（頭脳）契約**
- 描画ループ: `libs/ssruntime/docs/30_core/30_render_loop.ja.md`
- 制限事項: `libs/ssruntime/docs/10_introduction/20_limitations.ja.md`
- マスク: `libs/ssruntime/docs/40_components/40_mask.ja.md`
- FrameData/DrawBatch スキーマ: `libs/ssruntime/SCHEMA.ja.md`、`libs/ssruntime/fbs/framedata.fbs`、`libs/ssruntime/fbs/effect_draw_plan.fbs`
- SoA アクセサ実装: `libs/ssruntime/src/lib.rs:1804-1906`、doc コメント `libs/ssruntime/src/ffi/mod.rs:385-620`
- C-API 一覧: `libs/ssruntime/src/ffi/mod.rs`

**WebGL 参考実装（描画設計の元）**
- 正典: `../SSPlayerForWeb/SSPLAYER_WEBGL_SPEC.md`
- レンダラ: `../SSPlayerForWeb/packages/ssplayer-webgl/src/SSWebGLRenderer.ts`
- シェーダ: `.../src/programs/PartsColorProgram.ts` / `MaskProgram.ts` / `SSShaderLib.ts`
- テクスチャ: `.../src/SSWebGLResourceManager.ts`
- コールドスタート雛形: `../SSPlayerForWeb/TYRANO_COLDSTART.md`

**現行 wgpu プロトタイプ**
- `ssplayer-wgpu/core/src/renderer.rs`（Normal/Mesh のみ、再編元）
- `ssplayer-wgpu/core/src/player.rs`（SSPlayer の土台）
- `ssplayer-wgpu/viewer/src/main.rs`（winit+wgpu ループ）

**全体方針**
- `REDESIGN_ASSESSMENT.ja.md`（頭脳/体、オーバーライド/ミキシング/ステートマシンの将来計画）
