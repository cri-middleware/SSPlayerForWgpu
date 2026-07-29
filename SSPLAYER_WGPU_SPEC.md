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
- **3層**: `ssruntime`(計算・既存) / **層2 描画**＝`SSRenderer`(device スコープ) ＋ `SSScene`(描画ターゲットスコープ) / `SSPlayer`(操作・**GPU リソース非所有**)。
- **幹の設計＝借用モデル**: wgpu の `Device`/`Queue`/`Surface` に加え **`SSRenderer` も**ホストが所有。`SSScene`/`SSPlayer` は借りて与えられたターゲットへ描くだけ → Bevy 等へ**ロックインなしで相乗り**。
- **1 シーンに複数プレイヤー（★P7 で確定）**: `SSPlayer` は再生木だけを持つ。pipeline / テクスチャ / 頂点アリーナは `SSRenderer`(device 単位) と `SSScene`(ターゲット単位) が共有所有する。**「1 canvas = 1 player」「1 window = 1 player」は禁止形**（§1.2）。
- **提供3面（ssruntime と同型）**: `rs`(rlib) / `js`(wasm-bindgen) / `ffi`(cbindgen・DX12/Vulkan/Metal)。
- **データ所有契約**: `.ssab` は Player 所有→ssruntime へ borrow ／ FrameData・SoA は ssruntime 所有→Player が borrow（§1.1）。
- **ssab は embedded / reference 両対応**（テクスチャ埋め込み／外部参照）。
- **進捗**: **P0**（3層リファクタ＋借用モデル）✅ ／ **P4**（インスタンス §7.1・エフェクト §7.2）✅ ／ **P3**（マスク・Shape）✅ ／ **P2**（色パリティ: varg・全12ブレンド・premul 厳密化）✅ ／ **P6**（統合シェーダ ss_shader 全13種）✅ ／ **P1**（ssab embedded/reference 両対応・**WebGPU/js**）✅ ブラウザ実機確認済 ／ **P5**（ネイティブ ffi・C-API ＋ Metal C++ viewer）✅ macOS 実機確認済、DX12/Vulkan viewer は未着手 ／ **P7**（シーン再設計：SSRenderer/SSScene 分離・複数プレイヤー・SoA パススルー）🚧 **進行中**——7-1〜7-4 ✅（**1 ターゲットに N プレイヤーが動く**）、7-5 以降が残り。対応バッチ = Normal / Mesh / Shape / Instance / Effect / Mask。**wgpu 29**（§2.2）。詳細は §12。
- **リポジトリ**: SDK monorepo から分離した独立リポ `ssplayer-wgpu`。SDK は git submodule（`SpriteStudio-SDK/`）として取り込み、`ssruntime`/`ssab` を **path 依存**で直リンクする（§2.2）。

---

## §0 目的と設計判断

### 0.1 目的（優先度つき）

| 優先 | 目的 | 成果物 |
|---|---|---|
| **最優先（土台）** | **Rust ネイティブ Player**（イディオマティックな Rust API） | 単一クレート `ssplayer-wgpu`（`internal`/`player` モジュール）。**任意の wgpu ホスト（自作アプリ / egui / macroquad / Bevy …）に相乗りできるエンジン非依存の土台**。以下2つはすべてこの Rust API の上に載る |
| **主（配布）** | **WebGPU 対応**（ブラウザ） | Rust API に wasm-bindgen を被せた単一 Rust→WASM バイナリ ＋ Web Component（再設計後の ssplayer-webgl と同型。§10） |
| **副（配布）** | デスクトップ各グラフィックス（DX12 / Vulkan / Metal）対応 | Rust API に cbindgen を被せたヘッダ付きライブラリ（cdylib / staticlib） |

**設計の順序**: まず**綺麗な Rust API を確定**し、WebGPU（wasm-bindgen）と FFI（cbindgen）はその薄いラッパとして設計する。Rust API が全インターフェースの根。

wgpu が単一コードベースから WebGPU / Vulkan / DX12 / Metal / GL を吸収するため、**描画コードは1つ**で全ターゲットに展開できる。これが本プレイヤーを wgpu で作る根拠。Rust API を wgpu ネイティブ（借用モデル）に設計すれば、wgpu を使う任意のホスト——自作アプリ・egui・macroquad・Bevy 等——へ**特定エンジンにロックインせず**相乗りできる。Bevy はその代表例にすぎない。

### 0.2 決定表（論点 / 決定 / 理由）

| 論点 | 決定 | 理由 |
|---|---|---|
| 立ち位置（最優先） | **Rust 用 Player が第一級**。他はその上に載せる | Bevy 等 Rust エコシステムへ展開する土台。綺麗な Rust API を先に確定してから枝を生やす |
| 三面提供モデル | **ssruntime と同じ rs / ffi / js を1クレートに集約**（`crate-type=["rlib","cdylib","staticlib"]` ＋ `ffi`/`web` feature）。層は `internal`/`player` モジュール | ssruntime 自身が単一クレート・多 crate-type の実証。別クレート分割は便益なくメンテコスト増 |
| クレート分割 | **単一クレート `ssplayer-wgpu`＋モジュール**。別クレートは viewer(bin) と将来の Bevy アダプタのみ | 別クレートが正当化される条件（別 semver publish / 独自リリース周期 / ssruntime 抜き再利用）に非該当。Bevy だけ独自リリース周期のため将来分離 |
| アーキテクチャ層 | **3層**（ssruntime / 層2 描画 / SSPlayer）をモジュールで表現。**層2 は `SSRenderer`(device スコープ) と `SSScene`(ターゲットスコープ) の2オブジェクト**（P7） | 頭脳（計算）と体（描画）を分け、体をさらに「描画」と「操作」に分離。層2 を2つに割るのは、wgpu のリソース有効範囲が device 単位と描画ターゲット単位で異なるため（§1.2） |
| GPU デバイスの所有 | **SSPlayer は Device/Queue/Surface を所有しない（借用モデル）**。`&Device`/`&Queue` を借り、与えられた描画ターゲットへ描く | **エンジン非依存の相乗り**を可能にする。自作 wgpu アプリ / egui / macroquad / Bevy いずれも同じ API で組み込める。これは特定エンジンのためではなく「正しい Rust wgpu プレイヤーの形」 |
| **GPU リソースの所有（★P7 で改定）** | **`SSPlayer` は GPU リソースを一切持たない**。pipeline / sampler / テクスチャは `SSRenderer`（device 単位・ホスト所有）、投影 uniform / 頂点アリーナは `SSScene`（ターゲット単位）。`SSRenderer` も **Device/Queue と同じくホストが所有して貸す** | 旧設計は SSPlayer が `SSInternalPlayer` を専有し、同一 device 上の2体目で pipeline 約39本・アトラス・バッファがまるごと重複していた。借用の対象を「device」から「device ＋ レンダラ」へ広げるだけで、借用モデルの思想はそのまま複数体に伸びる |
| **1 ターゲット複数プレイヤー（★P7 で確定）** | **`SSScene` が描画ターゲット1枚に N 体を載せる**。`SSPlayer` は transform / order / visible / alpha を持ち、シーンが order 昇順で1 pass にまとめて描く | WebGL 版の再設計（`SSSurface` が players[] を持つ）と同型。wgpu は pipeline が **device 単位**なので、GL の program（context 単位）より共有範囲が広く、複数 canvas 間でも1式で済む |
| 命名（`SSSurface` を使わない） | 描画ターゲット単位のオブジェクトは **`SSScene`**。WebGL 版の `SSSurface` に対応するが名前は変える | `wgpu::Surface` と名前衝突する。読み手が「どちらの Surface か」を毎回考える羽目になるので、ここだけ参照実装と名を違える |
| **テクスチャの所有（★P7）** | **プレイヤー側は所有しない**。`SSRenderer` は供給された `TextureView` を**借りて** bind group を作るだけ（`set_texture`）。`upload_texture` は自前で作る便利経路にすぎない。`SSPack` は CPU データ専用 | 所有すると engine のアセット管理と**二重の所有系**になる。Bevy は `Assets<Image>`/`RenderAssets<GpuImage>` がライフサイクルを握っており、第二の所有者は二重アップロード・解放タイミングのずれ・フォーマット方針の奪取を必ず引き起こす。Device/Queue を借りるのと同じ理屈をテクスチャにも適用する（§8.2） |
| ホスト薄ラッパ（デバイス所有） | **viewer / web / ffi が各々 wgpu 一式を生成・所有**し、借用モデルの SSPlayer を駆動 | 標準 Rust アプリ・ブラウザ・C ホストは自前でウィンドウ/canvas を持つ。所有はそこに閉じる |
| Bevy（ロックインしない） | **コア（internal/player）に Bevy 依存を入れない**。Bevy 統合は任意・別クレート `ssplayer-wgpu-bevy`（既定でワークスペース無効） | 借用モデルが既にエンジン非依存＝Bevy は"代表的なホストの一例"。別クレート化でロックインを構造的に防ぐ |
| ssruntime の取り込み方 | **Rust クレートとして直接リンク**（C-API 経由ではない） | WebGL 版の「WASM＋別JSレンダラ」2部構成と違い、全 Rust の単一バイナリ。配布もデバッグも素直 |
| 描画モード | **`RenderMode::Direct`（既定）** | Player 推奨。FrameData の頂点シリアライズをスキップし SoA バッファ経由で取得（ゼロコピー寄り） |
| JS 露出 API（★P7 で改定） | **再設計後の ssplayer-webgl（`SSSurface` ＋ N 体の `SSPlayer`）と同型**にする。旧 `<ss-player>` の「1 canvas = 1 player・`play(name)` 1コール」には**揃えない** | 揃える相手が動いた。WebGL 版は 1 canvas = 1 player を捨てて surface+players へ再設計済みで、旧インターフェースは既に参照実装に存在しない。ドロップイン差し替えの目標は維持するが、その対象は**再設計後の API** |
| コールドスタート doc | **単一正典 `SSPLAYER_WGPU_SPEC.md`（本書、リポジトリ内 `ssplayer-wgpu/` 直下）** | WebGL 版 `SSPLAYER_WEBGL_SPEC.md` と対称。分かりやすい |
| 最初のマイルストーン | **3層リファクタ優先** | 現プロトの `Player`/`Renderer` を internal/player/ffi へ再編し土台を固めてから機能追加 |
| 検証環境 | **Rust ネイティブ viewer 1本 ＋ C++ 確認 viewer 3種（DX12/Windows・Metal/macOS・Vulkan/Linux）＋ Web 例** | ネイティブ C-API を per-backend 実機で検証するハーネス |
| ssab テクスチャ形態 | **reference / embedded の両対応**（経路を選ばず両方受ける） | ssconverter は「外部参照ssab」も「テクスチャ埋め込みssab」も出す。Player は両方をロードできる必要がある |
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
   │ 層2a【描画資源 / device スコープ】 SSRenderer（Rust + wgpu, std）        │
   │   pipeline（遅延生成・(blend,stencil,formats) キー）・bind group layout   │
   │   ・sampler・白フォールバック・**テクスチャキャッシュ（pack 単位で共有）**  │
   │   ★フォーマット非依存。1 device に1個あれば足り、全 Scene / Player が共有 │
   └──────────────────────────────┼──────────────────────────────────────┘
                                  │
   ┌──────────────────────────────┼──────────────────────────────────────┐
   │ 層2b【描画 / 描画ターゲットスコープ】 SSScene（Rust + wgpu, std）        │
   │   投影 uniform ＋ **フレーム頂点アリーナ（全 Player 共有の1本）** ＋      │
   │   draw_call 列 ＋ ステンシル管理。order 昇順で N 体を1 pass に積む        │
   │   update(&mut renderer,&device,&queue,dt,&mut [players]) / draw(&renderer,&mut pass)│
   └──────────────────────────────┼──────────────────────────────────────┘
                                  │
   ┌──────────────────────────────┼──────────────────────────────────────┐
   │ 層3【操作 / Rust API・第一級】 SSPlayer（Rust, std）                     │
   │   再生木（ノード毎 ssruntime Context）＋ ssab 保持 ＋ transform/order/    │
   │   visible/alpha。★GPU リソースを一切持たない（＝いくつ作っても安い）      │
   │   load / set_animation / play / pause / stop / speed / set_transform ...   │
   └──────────────────────────────┼──────────────────────────────────────┘
       借用（&Device, &Queue, &mut SSRenderer, &target）で駆動 ↑ ← ホストが所有
   ┌──────────────────────────────┴──────────────────────────────────────┐
   │ ホスト薄ラッパ（wgpu Instance/Device/Queue/Surface ＋ SSRenderer を所有）│
   │   ① rs  : viewer（winit）/ Bevy プラグイン（Bevy の RenderDevice を渡す）│
   │   ② js  : web feature（wasm-bindgen, canvas→surface, SsSurfaceWeb）      │
   │   ③ ffi : cbindgen C-API（raw-window-handle→surface, DX12/Vulkan/Metal） │
   │   ★device は**ホスト/ページで1つ**。canvas やウィンドウ毎に作らない      │
   └───────────────────────────────────────────────────────────────────────┘
```

- **1 再生ノード ＝ 1 ssruntime Context**。SSPlayer はそのノードの**木**を持つ（インスタンス子ごとに独立 Context、§7.1）。
- ssruntime は**ローカル座標**で「初期値＋変化点をマージ済み」の値を渡す。反転補正・座標系(Y-Up/Down)変換は**ssruntime 側で完結**しており層2 は不要（`local_vertices` にすべて反映済み）。
- **借用モデルが幹**: `SSScene`/`SSPlayer` は `&Device`/`&Queue`/`&mut SSRenderer` を借り、与えられた描画ターゲット（`&TextureView` / `&mut RenderPass`）へ描くだけ。`Device`/`Queue`/`Surface`/`SSRenderer` の所有は**ホスト**（viewer / Bevy / web / ffi）に閉じる。これにより Bevy のように自前の wgpu コンテキストを持つエンジンへ**そのまま相乗り**できる。
- **パイプラインはフォーマット依存だが `SSRenderer` はフォーマット非依存**。`(blend, stencil_test, color_format, stencil_format)` をキーに**遅延生成**してキャッシュするので、レンダラ自体は device に1個で足り、フォーマットの違う複数ターゲットが同居できる。

### 1.1 データ所有・ライフタイム契約（★厳守）

所有権の向きが**層で逆になる**。ここを取り違えると use-after-free になる。

| データ | 所有者（責務を持つ） | 参照側 | 渡し方 |
|---|---|---|---|
| **`.ssab`（アニメリソース）** | **Player（SSPlayer）** | ssruntime | Player が `SsabData`/`SsabResource` を保持し、ssruntime へは **borrow で渡す**（`bind_resource`）。ssruntime はこのリソースを所有しない |
| **FrameData / SoA バッファ** | **ssruntime** | Player | ssruntime が内部バッファに所有。Player は `get_frame_data` の戻り（`&[u8]` / SoA ポインタ）を **borrow として参照するだけ**。コピー・所有しない |

帰結（実装規約）:

- **ssab は Player より長生き**でなければならない。`SsabResource` を `Context`（ssruntime）より先に破棄しない。プロトは `Context<'static>` ＋ Player 保持の `Arc<SsabResource>` でこれを担保している（`libs/ssplayer-wgpu/src/player.rs`）。
- **FrameData / SoA の借用は次フレームまで**。`get_frame_data` を呼ぶたびに ssruntime 内部バッファが書き換わるので、Player は**そのフレーム内で使い切る**（保持しない）。マルチスレッド描画では SoA ポインタの use-after-free に注意し、描画ジョブ完了を待ってから `update`/`destroy`。
- この契約は REDESIGN_ASSESSMENT §4 の「FlatBuffers をポインタ渡し／コピーなし」と一致。層2 は借用した FrameData/SoA を GPU バッファへ**書き出した時点で借用を手放す**（GPU バッファ側が実体を持つ）。§5.5 の SoA パススルーはこの契約そのもの——borrow したスライスを memcpy して手放す。

### 1.2 GPU リソースのスコープ契約（★厳守 / P7）

§1.1 が「ランタイムとのデータ所有」を定めるのに対し、こちらは**プレイヤー内部の GPU リソース所有**を定める。原則は一つ：**リソースは「有効範囲がいちばん広いオブジェクト」が持つ。**

旧設計はこれを守らず `SSPlayer` が `SSInternalPlayer` を専有していたため、同一 device 上の2体目でパイプラインもアトラスもまるごと重複していた。

| リソース | スコープ | 所有者 | 重複したときの実害 |
|---|---|---|---|
| pipeline / bind group layout / sampler / 白フォールバック | **device** | `SSRenderer`（ホスト所有） | 12ブレンド×stencil3＋mask で約39本 × 体数。起動時のコンパイル時間とメモリ |
| アトラステクスチャ（hash → view + bind group） | **device** | `SSRenderer` が**参照を保持**（実体の所有は供給側。engine 統合時は engine のアセット系、§8.2） | 2048² アトラス1枚 = VRAM 16MB × 体数。同じキャラを2体出すだけで倍 |
| 復帰用のテクスチャ元データ | **供給元** | `upload_texture` 経路のみ `SSRenderer`。`set_texture`（借り物）は供給側の責務 | CPU 側にも 16MB × 体数（旧設計は premul 一時コピーと合わせて2重） |
| 投影 uniform / 頂点・インデックスアリーナ / draw_call 列 | **描画ターゲット** | `SSScene` | ターゲット毎に必要なので重複ではない。ただし **Player 毎に持つと player 境界をまたいだ draw call マージが原理的に不可能**になる |
| 再生木（Context / Slot / mask_flags）・transform・order | **プレイヤー** | `SSPlayer` | ここだけが本来 player 毎 |
| `.ssab` バイト列 | **pack** | `SSPack`（`Arc`、CPU のみ） | 同じキャラ N 体で N 部 |

帰結（実装規約）:

- **`SSPlayer` に wgpu 型のフィールドを置かない。** 置きたくなったら、それは `SSRenderer`（device 単位）か `SSScene`（ターゲット単位）のどちらかに属する。この規約が「1 canvas = 1 player」の再発を構造的に防ぐ。
- **device はホスト／ページに1つ。** canvas やウィンドウごとに `request_device` してはいけない。device をまたぐとテクスチャも pipeline も共有できず、`restore_device` も体数ぶん走る。
- **`SSScene` は Player を所有せず借りる**（`&mut [&mut SSPlayer]`）。ECS のように Player がホスト側のデータ構造に住む場合にそのまま載るため。「シーンが players を所有する」ぶんの利便性は、所有ホスト（`SsSurfaceWeb` / `NativeHost` / C-API）が `Vec<SSPlayer>` を持って提供する。
- **Rust 実装メモ**: `SSScene::update` は自分のアリーナと借りたレンダラの両方に `&mut` が要る。`let Self { arena, projection, .. } = self;` のフィールド分解で借用を分離する（`self` の2フィールドを同時に `&mut` 借りしようとして詰まるのが定番）。

---

## §2 最終プロジェクト構成

**単一クレート `ssplayer-wgpu`（`libs/ssplayer-wgpu/`）に3層をモジュールで集約する。**（✅=P0 実装済 / ◻=将来）

**ディレクトリ規約**: ライブラリクレートは `libs/<クレート名>/`（SpriteStudio-SDK と同じ規約）、ホスト実装・デモは `examples/<プラットフォーム>/`。Rust か否かは問わない（`examples/web` は JS、C++ viewer は CMake）。

```
ssplayer-wgpu/
├── SSPLAYER_WGPU_SPEC.md          ← 本書（正典）
│
├── libs/                          ← ★ライブラリクレート置き場（ディレクトリ名 = クレート名）
│   ├── ssplayer-wgpu/             ← ★単一ライブラリクレート（rs/js/ffi を集約）
│   │   ├── Cargo.toml             [lib] crate-type = ["cdylib","rlib","staticlib"]
│   │   │                          features: web(wasm-bindgen) / ffi(cbindgen)。Device は所有しない
│   │   ├── build.rs               ✅ ffi feature 時のみ cbindgen を実行（無効時は no-op）
│   │   ├── cbindgen.toml          ✅ 生成ヘッダの体裁（型名を ss_player_t 等へ rename）
│   │   ├── include/               ✅ 生成物 ssplayer_wgpu.h（CMake 側が参照）
│   │   └── src/
│   │       ├── lib.rs             ✅ 層の公開（SSRenderer / SSScene / SSPlayer が主 API）
│   │       ├── frame.rs           ✅ FrameView … ランタイム所有データへの借用（§1.1）
│   │       ├── renderer.rs        ◻ P7-4: 層2a SSRenderer の置き場。**型は 7-3 で実装済**
│   │       │                         （現状 internal.rs 内）。ファイル分割は scene.rs と同時
│   │       ├── scene.rs           ◻ P7: 層2b SSScene（ターゲットスコープ。投影 ＋ 頂点アリーナ
│   │       │                         ＋ draw_call ＋ ステンシル管理 ＋ order 合成）§9.2
│   │       ├── pack.rs            ◻ P7: SSPack（.ssab の共有単位。★CPU のみ・GPU 非所有）§8.2
│   │       ├── internal.rs        ✅ 7-3 で `SSRenderer`(device) と `SSInternalPlayer`(ターゲット)
│   │       │                         の2型に分割済。7-4 で2ファイルへ引っ越す
│   │       ├── player.rs          ✅ 層3 SSPlayer（操作。再生木 ＋ transform/order/visible）
│   │       │                         ★P7 以降 wgpu 型のフィールドを持たない（§1.2）
│   │       ├── mask.rs            ✅ MaskPlanner（§6）
│   │       ├── web.rs             ✅ #[cfg(feature="web")] wasm-bindgen（② js）
│   │       │                         ◻ P7: SsSurfaceWeb（1 canvas = N player、device はページ共有）
│   │       ├── host.rs            ✅ #[cfg(feature="ffi")] NativeHost（生ハンドル→wgpu 一式を所有）
│   │       │                         ◻ P7: 1 window = N player（Vec<SSPlayer> を持つ）
│   │       └── ffi.rs             ✅ #[cfg(feature="ffi")] extern "C" ss_scene_* / ss_player_*（③ C/C++）
│   │                                 ★ABI 変換のみ。wgpu ロジックは host.rs、再生は player.rs
│   └── ssplayer-wgpu-bevy/        ◻【将来】Bevy アダプタ（借用モデルの一利用例）
│                                     ★コアはこれに依存しない。独自リリース周期のため別クレート
│                                     Bevy の wgpu メジャーが 29 と食い違う場合は
│                                     `exclude` して独立ワークスペース化する（§2.2）
│
└── examples/                      ← ホスト実装・マルチプラットフォーム動作確認環境
    ├── viewer/                    ✅ Rust ネイティブ viewer（bin, ① rs のホスト例）
    │   └── src/main.rs            winit + wgpu 一式を所有し、借用モデルの SSPlayer を駆動
    ├── web/                       ✅ WebGPU ブラウザ確認（<ss-player-webgpu>、② js）
    ├── viewer-dx12-cpp/           ◻ C++ DX12 確認 viewer（Win32 HWND → C-API → backend=DX12）
    ├── viewer-metal-cpp/          ✅ C++ Metal 確認 viewer（Cocoa CAMetalLayer → C-API → Metal）
    │   ├── CMakeLists.txt         cargo build --features ffi を叩き、cdylib＋生成ヘッダを消費
    │   └── src/main.mm            Objective-C++。NSWindow/CAMetalLayer を所有し C-API を駆動
    ├── viewer-vulkan-cpp/         ◻ C++ Vulkan 確認 viewer（Linux Xlib/Wayland → C-API → Vulkan）
    └── bevy/                      ◻【将来】Bevy アダプタのデモアプリ
```

> js（`web`）は feature、ffi は feature ＋ crate-type 追加で**同一クレート内**に載る（ssruntime と同型）。`libs/` に増えるのは将来の Bevy アダプタのみで、それ以外の面はすべて `ssplayer-wgpu` 1クレートに閉じる。

**C++ 確認 viewer 3種は「共通コードを括り出さず、各ディレクトリ自己完結」とする。** 3者が違うのはウィンドウ／サーフェス生成（HWND / CAMetalLayer / Xlib・Wayland）だけで、C-API を叩く再生ループ自体は数十行しかない。共通ライブラリに逃がすとサンプルの価値である「1ディレクトリまるごとコピーすれば動く」が失われるので、その数十行は意図的に重複させる。`examples/` が増えて見通しが悪くなったら `examples/cpp/{dx12-win32,metal-macos,vulkan-linux}/` へまとめ直す。

### 2.1 Cargo ワークスペース登録

ルート `Cargo.toml` の `members` は `libs/ssplayer-wgpu`（クレート `ssplayer-wgpu`）と `examples/viewer`（クレート `ssplayer-wgpu-viewer`）。**バージョンは crate ごとに独立**（ライブラリと開発用 viewer は別物なので `workspace.package.version` は置かない）。将来 Bevy を足す時だけ `libs/ssplayer-wgpu-bevy` を追加する。C++ の examples は Cargo ワークスペース外で、CMake が cbindgen 生成ヘッダ＋ビルド済みライブラリを消費する。

### 2.2 SDK submodule と wgpu 版

**SDK は git submodule `SpriteStudio-SDK/`**。他の Player（Godot/Unity/RenPy/Web/Flutter）が C-API をリリース済み zip 経由で使うのに対し、本 Player だけは **ssruntime / ssab の Rust API を直接リンク**する（`root_as_ss_anime_binary_unchecked` などの FlatBuffers 型を読む）ため、未リリースの ssab スキーマ変更に追随する必要がある。`SSConverterGUI` と同じ構成。

- ルート `Cargo.toml` の `[workspace] exclude = ["SpriteStudio-SDK"]` は必須。これが無いと cargo が submodule 内の SDK クレートを**本ワークスペースのメンバーとして掴み**、SDK 側の `workspace.dependencies` 継承が解決できず manifest エラーになる。
- 依存は `[workspace.dependencies]` に集約：`ssruntime = { path = "SpriteStudio-SDK/libs/ssruntime" }` / `ssab = { path = "SpriteStudio-SDK/libs/ssab", default-features = false }`。
- **wgpu は 29 系にピン**。Vtuber アプリで Slint と同一デバイスを共有する構成があり、Slint の `unstable-wgpu-NN` は wgpu メジャーの完全一致を要求するため。**メジャーを上げるときは必ず Slint 側の対応メジャーを確認する**。ワークスペースで1つの wgpu しか解決できないので、これは repo 全体の制約。

**30 → 29 の API 差分（ダウングレード時に触った箇所）**: `VertexState.buffers` は `&[VertexBufferLayout]`（30 は `&[Option<_>]`）／ present は `SurfaceTexture::present()`（30 は `Queue::present(tex)`）／ `RequestAdapterOptions` に `apply_limit_buckets` は無い／ `SurfaceConfiguration` に `color_space` は無い／ `BufferSlice::get_mapped_range()` は `BufferView` を直接返す（30 は `Result`）。`get_current_texture() -> CurrentSurfaceTexture`（enum）は 29 にもある。

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

### 3.3 層2（描画）が担う責務（＝ ssruntime 対象外）

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
**✅ 全12種を起動時に事前生成**（`internal.rs` `BLEND_ORDER` / `blend_state`、premul 係数）。GPU ネイティブ等価のない `Div2/Overlay2` は Mix、`Mulalpha/Mul2` は Mul、`Screen2` は Screen へフォールバック（ssplayer-pixi 準拠、要検証分は SS6PlayerForUnity と照合予定）。stencil 有効時は各ブレンド × 3（None/Inside/Outside）。

---

## §5 層2（描画）の設計

### 5.1 頂点フォーマット（WebGL 版と同一 SoA 意味論）

| 属性 | loc | 型 | 内容 |
|---|---|---|---|
| position | 0 | vec2/vec3 | ワールド座標（Normal は世界行列適用後、Mesh は済み＋z） |
| uv | 1 | vec2 | 正規化 UV（0..1） |
| color | 2 | vec4 | RGBA 0..1（パーツカラー頂点色 or α） |
| varg | 3 | vec4 | `[rate, isMix, isMul, isAddSub]`（PartsColor ブレンド制御） |

> **✅ 実装済**: `Vertex{position:[f32;3], uv, color, varg:[f32;4]}`（loc 3）。varg は PartColor の**自前 blend_type**（draw batch の blend ではない）から導出：Normal は per-corner `[rate, isMix, isMul, isAddSub]`、Mesh は corner-0 単一、Shape は Mix 固定 `[hasPc?1:0, 1,0,0]`、Effect / PartColor 無しは `[1,0,1,0]`（mul modulate）。PartColor blend は Mix/Mul/Add/Sub のみ有効、他は Mix クランプ（`parts_color_flags`）。

Normal パーツは builder が index を出さないので、5頂点（TL,TR,BL,BR,center）に対し中心を要にした扇 12 index を張る：`(0,1,4)(1,3,4)(3,2,4)(2,0,4)`。現プロトは4隅2三角形だが、頂点変形/カラーで center が効く場合に扇へ切替。

**★P7 で改定（頂点を詰める）**: 現行 52 B/頂点には無駄がある。

- **`position.z` は完全に死んでいる**。投影行列の第2列は `[0,0,1,0]`（`update_projection`）なので z は x,y に影響せず、`vs_main` は `out.clip_position = vec4(pos.x, pos.y, 0.5, 1.0)` と **z を捨てて 0.5 を直書き**している。つまり z 成分の変換（3 mul + 2 add / 頂点）と 4 B/頂点は計算して捨てているだけ。→ **`position: vec2` にする**（Mesh の `position_z` も同様に不要）。
- `color` は 0..1 の f32×4 → **`unorm8x4`（4 B）**で十分（元データが 0..255 の u8）。
- `varg` は実質 `(rate: f32, mode: 2bit)` → **packed u32（4 B）**。

結果 **pos(8) + uv(8) + color(4) + varg(4) = 24 B**（現状比 46%）。ただしこれは §5.5 の SoA パススルーへ移行するまでの中間形であり、最終形では**頂点バッファ自体が消える**。

### 5.2 パイプライン設計（WebGL の動的 state → wgpu は焼き込み）

**wgpu では BlendState / DepthStencilState は RenderPipeline に焼き込む**。よって WebGL の `applyBlend`（動的 `blendFuncSeparate`）は消え、代わりに **組合せ分のパイプラインを事前生成して差し替える**。

```
パイプライン ≈ BlendType(12) × 用途{parts, effect, mask-write, mask-test, ss-shader×13}
> **✅ 実装済**: parts/effect（P2, 12ブレンド×stencil3）＋ mask-write/test（P3）＋ ss-shader 13種（P6, `(id_hash,blend)` 毎に遅延生成）。
```

**★P7 で確定：全パイプラインを遅延生成にする。** 現行は `SSInternalPlayer::new` が 12ブレンド × stencil3 = 36本＋マスク書き込み3本を**起動時に全部**コンパイルしており、1ブレンドしか使わないアニメでも全額払っている（初回表示レイテンシは §14 の WebGPU 弱点そのもの）。統合シェーダ側（`ss_pipelines`）が既に `(id_hash, blend)` キーの遅延生成を採用しているので、**同じ手法を content / mask 側にも広げる**だけ。

- キーは **`(blend, stencil_test, color_format, stencil_format)`**。フォーマットをキーに含めることで `SSRenderer` 自体がフォーマット非依存になり、**1 device に1個**で足りる（§1.2）。
- 20体を載せたページで旧設計は 39本 × 20 = 780本、新設計は実使用ぶんの数本。
- 去重は「同一パイプラインなら再バインドしない」（現プロトの `_lastBlend` 相当）。

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

alpha は全モード `src=One, dst=OneMinusSrcAlpha`（premul 前提）。**✅ 実装済**（`blend_state`）。**premul 厳密化**: テクスチャは `upload_texture` で CPU premultiply（WebGL の `UNPACK_PREMULTIPLY_ALPHA_WEBGL` 相当）、シェーダは逆premul→カラー演算→再premul、ブレンド係数も premul の三位一体。既存フィクスチャ（Mask/Shape）のピクセル数は P2 前後で不変を確認（premul は完全不透明/フラット色では往復ロスレス）。

### 5.3 カラーブレンド WGSL（WebGL の `PartsColorProgram` を直訳）— ✅ 実装済（`internal.rs` `fs_main`）

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

**投影は `SSScene`（ターゲット単位）、配置は `SSPlayer`（体単位）**（P7）。現行はルート transform が単位行列固定で、位置決めの手段が `set_viewport` しかない——複数体を1画面に置く前提が無かったことの現れ。`render_node` は既にインスタンス子のために任意 transform の合成（`mat4_mul`）を実装済みなので、**ルートに Player の行列を渡すだけで動く**（配管は揃っていて公開されていないだけ）。

### 5.5 頂点生成経路（★P7・WebGPU 固有の本命）

**現状は「WebGL でできることを wgpu で書いた」形**——毎フレーム CPU で `Vec<Vertex>` を組み立て（パーツ×4頂点×13 float の**散らばったストア**）、`write_buffer` で上げ、`draw_call` を1本ずつ encode している。ここが CPU 律速で、WebGL 版と同じ天井に当たる。

**WebGPU には storage buffer があるので、頂点を CPU で組む必要がない。**

ssruntime は `world_matrices` / `local_vertices` / `local_uvs` / `mesh_*` を**フラットな f32 配列**で出している（§3.2）。これを**そのまま GPU へ渡し、頂点シェーダで展開する**：

```
[CPU] ノード毎: SoA スライスを scene アリーナへ copy_from_slice（SIMD memcpy）
      パーツ毎: {arena 内 part slot, node transform index, color, varg} の小さなレコード
      draw 毎 : {pipeline, texture, 範囲}
[GPU] vs: world_matrices[part*16] * local_vertices[part*10 + corner]
```

| | 現行 | SoA パススルー |
|---|---|---|
| CPU の書き込み | パーツ × 4頂点 × 13 float の散らばったストア | **数回の memcpy** |
| 転送量 | 208 B（頂点）+ 24 B（index）= 232 B/パーツ | 144 B/パーツ（matrix 64 + verts 40 + uvs 40）※更に圧縮可 |
| index buffer | **毎フレーム再構築＋再アップロード** | quad は `0,1,2,0,2,3…` の**静的パターン**→ 一度作れば再アップロード不要、または indexless `draw(0..6n)` |

- **§1.1 の所有契約そのもの**：SoA は ssruntime 所有・Player は borrow。memcpy はまさに「borrow して書き出した時点で手放す」。
- インスタンスサブツリーの transform 合成は**ノード単位**（パーツ単位ではない）なので、ノード transform 配列を別 storage buffer に置いて index 参照すれば `mat4_mul` の回数は変わらない。
- mesh / shape も同じ扱い（`mesh_vertices_x/y` / `mesh_uvs` / `mesh_indices` は既に SoA）。エフェクトの `EffectDrawPlan` も同形。
- **WebGL2 は SSBO 無し・UBO 64KB・大規模動的 index 不可**なので、この形には到達できない。ssplayer-webgl と差がつく最大の一点であり、P7 で WebGPU を選ぶ意味そのもの。
- これが入ると Player 単位の alpha / tint も「per-player レコード1本」で済む（現行の形だと全頂点書き込みに触る必要がある）。**機能追加より先にこの経路へ移すこと。**
- 段階の先には **render bundle**（draw_call 列の再 encode を省く）と **timestamp query**（バッチ単位の GPU 実測。WebGL の `EXT_disjoint_timer_query` は Spectre 対策でブラウザから実質消滅している）がある。どちらも draw_call 構造が安定してから。

---

## §6 マスク（ステンシル）✅ 実装済（`libs/ssplayer-wgpu/src/mask.rs` ＋ `internal.rs`/`player.rs`）

- **`MaskPlanner`（WebGL 版 core・エンジン非依存）の rank 昇順スコープ replay ロジックを Rust に移植**（`libs/ssplayer-wgpu/src/mask.rs`：`plan_writers`/`MaskWriter`/`PartMaskFlags`/`StencilTest`/`MaskOp`）。ライタごとに rank / OP(INCR|INVERT) / cutout閾値 / clipping フラグを持つ。**ライタの texture は所属 DrawBatch から取る**（pure Mask batch は `tex=0`→閾値 -1＝カットアウト無しで全ジオメトリ、clipping は content の実 texture）。
- **wgpu では stencil op もパイプラインに焼く**：write 用（IncrementClamp/DecrementClamp/Invert）と test 用（NotEqual/Equal, `stencil write_mask=0`）で別パイプライン。ステンシル ref は常に 0（ops/compare をパイプラインに焼くため動的変更不要）。content パイプラインは `blend×stencil_test(None/Inside/Outside)` のマトリクス、カットアウト閾値は mask-write 頂点の `color[0]` に載せ `fs_mask` が discard。
- 手順（`40_components/40_mask.ja.md` 準拠）:
  1. pure マスクを事前書き込み（コンテンツ描画前に全ステンシル書き込み、`ColorWrites::empty()`）。
  2. コンテンツを rank 昇順で描画しつつ、`sync_masks_to(rank)` で clipping ライタ追加 / pure ライタ除去。
  3. マスク対象は polarity 判定（`visible_inside_mask` → `NotEqual,0`(内側) / `Equal,0`(外側)）。polarity がバッチ内で変わるため現行は**パーツ毎描画**。→ **★P7: polarity は静的な per-part 属性なので、同一 polarity の連続 rank を**ランレングス結合**して draw call を畳む（描画順は保存されるので安全）。pure マスク混在バッチ（色スキップのために1パーツ毎に落ちている経路）も同様。
  4. **pure マスク（`PartTypeMask`/`draw_as_mask`）は色を描かない**：runtime は pure マスクにも content batch を発行するので、`render_node` で `pure_mask()` パーツの描画をスキップ（§3-3）。

- **★ステンシルの不変条件（P7・複数プレイヤーの前提条件）**: **各 Player は、ステンシルバッファを見つけた時と同じ状態にして返す。**
  - 現行の `render_node` は writer を rank 昇順に `sync_masks_to` で消化していくが、**バッチループを抜けた時点で未消化の writer を後始末しない**。pure マスクは冒頭で無条件に `Incr` 済みなので、対応する `Decr` に到達しなかったぶんのカバレッジがフレーム末尾に残留する。
  - 今これが顕在化していないのは **1 pass = 1 player で pass 冒頭の `LoadOp::Clear(0)` に毎回救われているから**にすぎない。同じ pass に Player A → B を積んだ瞬間、**A のマスクが B をクリップする**。
  - 対処は**末尾で残り writer を全て sync（完全巻き戻し）**。追加コストは実質ゼロ。pass 分割は高くバッチも切れる、ステンシル 8bit の分割は 8体上限で論外。
  - 副次効果：Player 境界でステンシルが必ず 0 と保証できるので、マスクを使わない Player は `StencilTest::None`（compare Always）のまま隣の Player とマージできる。
- **インスタンス継承**（§7.1 木の再帰に `inherited_stencil` を伝播）：被マスク対象がインスタンスなら子アニメ全体を親ステンシルでクリップ（screen-space・単段、`40_mask.ja.md §3-2-2`）。
- **サーフェス/描画ターゲットに stencil 付きデプスバッファを確保する**必要がある。`stencil_format: Option<TextureFormat>` で opt-in（現行 `SSPlayerConfig`、P7 以降は `SceneConfig`＝ターゲット単位が正しいスコープ）。viewer は `Depth24PlusStencil8` を確保し render pass に attach、`None` ならマスク無効で回帰なし。
- **白フォールバックテクスチャは必ず白で初期化**：1x1 テクスチャは `create_texture` だけでは透明黒（ゼロ初期化）で、Shape/テクスチャ欠落 content が `tex*color=0` で不可視になる。`new()` は借用モデルで queue を持たないため、`end_frame` 初回に `queue.write_texture([255;4])` で遅延初期化（`white_ready` フラグ）。
- 既知の限界: INCR と INVERT 混在の重なりマスクは単一ステンシル値で近似（厳密解は coverage-bitmap 方式）。子アニメ内部定義のマスクは非適用（WebGL 版と同一）。エフェクト被マスクは content パイプライン流用で対応済。
- 検証: `libs/ssplayer-wgpu/tests/mask.rs`（planner probe、Mask.ssab の writer/Shape-mask 分類）＋ `libs/ssplayer-wgpu/tests/render.rs`（ヘッドレス GPU offscreen readback：masking on/off の差分・Shape content の描画量で回帰検出）。

---

## §7 エフェクト / インスタンス（Slot API）

Godot / Unity のネイティブプレイヤー実績と C-API 契約から、この節の設計は確定している。

### 7.1 インスタンス＝「子ごと独立 Context」の再生木（★確定）

- **各インスタンス子アニメに独立した Context（＝独立した SoA バッファ）を割り当てる。** これが SoA 単一バッファ上書き問題の構造的解法で、`ss_instance_slot_step(slot, ev, child_ctx, ...)` が **child_ctx を引数で要求する** C-API 契約そのもの。子の `get_frame_data` は子 Context の SoA に書くので親を汚さず、**再取得もコピーも不要**。
- **再帰の単位は「再生ノード」＝ Context ＋ 各種 Slot ＋ 子ノード群**。SSPlayer はこれを木として所有する（親→子→孫）。InstanceSlot / EffectSlot は各ノードの再生層が part_index で保持。子 Context が独立なので**全ノードの FrameView が同時に有効**であり、順に層2 へ渡せる。
- **描画層は木に含めない（案A）。** GPU リソースは木の外——pipeline/texture は `SSRenderer`（device 単位）、累積頂点アリーナは `SSScene`（ターゲット単位）——にあり、木全体どころか**シーン内の全 Player で共有**する（§1.2）。テクスチャは hash がグローバルなので子アトラスも同じマップに載る。描画層は Context を一切持たず、渡された FrameView 群を消費するだけ。
- **描画順**: 子は親の Instance パーツの rank 位置に一塊として差し込む（親パーツ列の途中に子が入る）。
- **wgpu の実装型は Unity 型**: 子はローカル空間で頂点/メッシュを中間バッファに事前ビルド → 親がその頂点を world 行列で変換して累積バッファへ merge（子の完了を依存に持つ）。Godot 型（子が自分の CanvasItem に先に描き親が RID 再ペアレント）は wgpu に retained scene graph が無いため移植不可。Context 独立を活かした `get_frame_data` の並列化も可能。
- ExternalInstance は参照先 ssab を SSPlayer が別途ロードし、その Context を子ノードとして木に接ぐ。

**実装メモ（✅ 実装済・`libs/ssplayer-wgpu/src/player.rs`）**

- `PlaybackNode { context, instances, effects }` の木を SSPlayer が所有。GPU リソースは現行 `SSInternalPlayer` 1個を共有（P7 で `SSRenderer` / `SSScene` へ移り、共有範囲が**シーン内の全 Player**へ広がる）。
- **外部参照は pack 名の文字列**（`PartTypeInstance::ref_anime_pack()`/`ref_anime_name()`）で解決＝ハッシュアルゴリズム不整合を回避。ホストは全 pack を `add_ssab(pack_name, data)` で事前登録し、`load_ssab(pack_name, data)` で主 pack 指定。ssab 型は `ssab::ssab::ss::format::{PartType, root_as_ss_anime_binary_unchecked}`。
- **transition スナップはホスト責務**: `InstanceSlot::step` は transition 時にセクション/ループ/方向を設定するのみで開始フレームへのスナップはしない。**`result.transitioned` 時に子 `context.play()` を呼ぶ**（呼ばないと独立モードが再生状態にならず進まない）。
- **`parent_looped` はランタイム非通知**: root は前フレームからのフレーム後退で検出（前向き再生のループ edge）、子は `result.child_looped` を伝播。
- 検証: `libs/ssplayer-wgpu/tests/instance.rs`（ヘッドレス probe。同期/独立/pingpong の frame 推移を確認）。

### 7.2 エフェクト（別 Context を持たない）✅ 実装済

- エフェクトは**別 Context 不要**（親 Context の frame_no/delta で駆動するシミュレータのため上書き問題が無い）。`EffectSlot<'static>` を再生ノードが part_index で保持。`EffectSlot::new(resource, PartTypeEffect::ref_effect_name_hash(), seed=part_index)`。
- `slot.step(ev, frame_no, delta_secs, fps, parent_looped, y_flip=false, vert_stride=2)` → `ss_effect_step_result{ visible, draw_plan_buf: *const u8, draw_plan_len }`。`draw_plan_buf` はスロット所有・次 step まで有効なので**即コピー消費**。`ev = ctx.get_active_effect_event(part_index)`、`fps = ctx.get_fps()`。
- `EffectDrawPlan`（`root_as_effect_draw_plan_unchecked`）: `commands`（`cellmap_hash`/`quad_count`/`particle_offset`/`blend` 0=Mix 1=Add）、`vert_stride`、`verts`/`uvs`/`colors`/`indices`。スライス式（`po=particle_offset`）: verts `[po*4*stride..(po+qc)*4*stride]` / uvs `[po*4*2..]` / colors `[po*4*4..]` / indices `[po*6..]`（**quad-local 0基点、rebase 不要**）。
- **★エフェクト UV は正規化済み(0..1)**: スプライト/メッシュの pixel UV と違い、**tex_size で割ってはいけない**（probe で確認・当初バグを修正）。colors も正規化。
- verts は **slot-relative** → エフェクトパーツの world 行列を transform 合成（`mat4_mul(transform, effectPartWorld)`）。
- **1 command = 1 draw call**（`cmd.blend` でパイプライン切替、`cmd.cellmap_hash` でテクスチャ）。エフェクトは PartsColor を持たず、plan の色が既存 tex*color シェーダを変調するので varg 不要。実装: `libs/ssplayer-wgpu/src/internal.rs::append_effect`。

---

## §8 テクスチャ管理

- **hash → `{texture, width, height}`** の Map。textureHash は atlas 識別子（fnv1a）。**★P7: この Map は `SSRenderer`（device スコープ）が持つ**——同じキャラを N 体出しても VRAM もアップロードも1回で済ませるため（§1.2）。WebGL 版が `SSWebGLTextureCache` を surface 側に置いたのと同じ判断で、wgpu は pipeline 同様テクスチャも device 単位なので**共有範囲はさらに広い**（複数 canvas 間でも共有できる）。
- **premultiply を必ず適用**（SS 必須）。ネイティブは RGBA デコード後に自前で premul、または premul 済みテクスチャを供給。
  - **★注意（WebGPU の弱点）**: WebGL の `UNPACK_PREMULTIPLY_ALPHA_WEBGL` はドライバが無料でやってくれるが、**WebGPU の `writeTexture` に等価は無い**。現行は `upload_texture` が CPU で premul しており、2048² で 16 MB の一時コピーが発生する。
  - **★P7 の対処（web）**: `Queue::copy_external_image_to_texture`（wgpu 29 にあり）＋ `createImageBitmap` を使えば、**デコードはブラウザのネイティブ実装（オフスレッド）、premultiply は GPU 側**に寄せられる。主目的はこの2点で、wasm から PNG デコーダ（`image` crate）が落ちるサイズ減はおまけ（§14.1 のとおりサイズは元々問題ではない）。
  - ネイティブ側は CPU premul のままだが、使い回しスクラッチバッファにしてアップロード毎の alloc を無くす。
- フィルタ/wrap: `ClampToEdge`（NPOT 対応）＋ `Linear`、mipmap なし。
- **texture_hash 0 / 未ロード → 1x1 白テクスチャ**にフォールバック（Shape content・純カラー対応）。**この白テクスチャは必ず白(255,255,255,255)で初期化すること**（`create_texture` だけだと透明黒で `tex*color=0` になり Shape が不可視）。`new()` は queue を持たないため `end_frame` 初回に `write_texture` で遅延初期化（§6）。

### 8.1 ssab の2形態に両対応（embedded / reference）— ✅ 実装済

ssconverter は `.ssab` を2形態で出力しうるので、SSPlayer はどちらでもロードできる必要がある。

| 形態 | 内容 | Player の扱い |
|---|---|---|
| **embedded** ✅ | テクスチャ（atlas PNG）を `.ssab` に埋め込み | `add_ssab` 時に **Player が cellmap を列挙 → 埋め込みバイト列をデコード**（CPU、`image`）、次の `update` で **premul → upload_texture** まで自動（自己完結、単一ファイル配布に最適・広告向け） |
| **reference** ✅ | テクスチャは外部ファイルを名前/hash で参照 | Player は hash だけ把握。**実バイトはホストが供給**（`upload_texture(hash, rgba, w, h)`）。ロード経路（ファイル/URL/バンドル）はホストの責務 |

- **ハッシュ対応（重要）**: `DrawBatch.texture_hash = CellMap.name_hash`（`framedata.rs:104`）。埋め込みバイト列は `embedded_assets`（key = `CellMap.image_path_hash`）にある。→ embedded ロードは **cellmap 毎に `embedded_assets[image_path_hash]` をデコードし、`name_hash` で upload** する（2つのハッシュを取り違えない）。
- **統一供給 API**: 最終的にはどちらも `upload_texture(hash, premul済みRGBA, w, h)` に集約される。embedded は Player 内部でこの呼び出しを行い、reference はホストが行う。ブラウザの `TexImageSource` 供給・ネイティブの RGBA 供給と対称。
- **借用モデルとの整合**: `add_ssab`/`load_ssab` は device/queue を借りないので、デコードは load 時（CPU）、GPU upload は次の `update`（queue 貸与時）に `pending_textures` を flush。
- **feature `embedded`（default on）**: Player 内蔵 `image`（PNG のみ）でデコード。WASM サイズを抑える場合は `--no-default-features` で切り、デコードをブラウザ（`createImageBitmap`）に委ねてホスト供給に一本化。
- **検証**: `render.rs::render_embedded_ssab`（埋め込み `Basic.ssab` をテクスチャ無供給でロード → reference レンダーとピクセル完全一致 14378/14378）。埋め込みフィクスチャは `assets/overall_emb/`（`ssconverter-cli --embedded` 生成）。

### 8.2 SSPack とテクスチャ供給（★P7）— **GPU テクスチャは所有しない**

「同じキャラを何体も出す」が普通になるので `.ssab` の共有単位を作る。ただし **`SSPack` は CPU データだけを持ち、GPU テクスチャは所有しない。**

```rust
pub struct SSPack { /* Arc<SsabResource>, 必要な texture hash 一覧, embedded 画像の取り出し口 */ }

SSPack::load(bytes) -> Result<SSPack, SSRuntimeError>   // ★device も queue も要らない（CPU のみ）
pack.required_textures() -> &[u32];                     // このパックが要求する hash
pack.decode_embedded() -> Vec<(u32, Vec<u8>, u32, u32)>; // embedded の場合のみ。ピクセルを**渡すだけ**

player.set_pack(&pack);   // Arc をクローンして参照するだけ
player.add_pack(&pack);   // 外部インスタンス参照解決用
```

**なぜ GPU テクスチャを持たせないか（★設計判断）**: 持たせると `SSPack` の refcount が **engine のアセット管理と二重の所有系**になる。Bevy なら `Assets<Image>` / `Handle<Image>` / `RenderAssets<GpuImage>` が既にライフサイクル・ホットリロード・アンロードを握っており、そこへ第二の所有者を持ち込むと (1) 同じアトラスが Bevy 側と我々側で**二重アップロード**、(2) 解放タイミングが engine の意図とずれる、(3) フォーマット・mip・サンプラの方針を engine から奪う、という3点で必ず衝突する。

**テクスチャは所有ではなく借用チャネルにする**（Device/Queue と同じ扱い＝借用モデルの一貫適用）:

```rust
// 供給側が所有する TextureView を借りる。renderer はこれで bind group だけ作る
renderer.set_texture(hash: u32, view: &wgpu::TextureView, size: (u32,u32), alpha: AlphaMode);

// 便利経路（viewer / web / ffi）: renderer が自分でテクスチャを作ってから set_texture する
renderer.upload_texture(&device, &queue, hash, rgba, w, h);
```

- **Bevy 統合**: アダプタが `GpuImage.texture_view` を毎回 `set_texture` で渡すだけ。二重アップロードなし、解放は Bevy の意志どおり。embedded ssab の場合も `decode_embedded()` の**ピクセルを `Assets<Image>` へ流し込めば**、テクスチャは最初から Bevy の資産になる。
- **wgpu の型は内部が refcount** なので `TextureView` を clone して保持しても dangling しない。ただし**我々が握っている間は解放が遅れる**——engine 側で不要になったら `renderer.forget_texture(hash)` を呼ぶ、という契約にする。
- **`AlphaMode`（premul / straight）を per-texture で持つ**。engine が供給するテクスチャは premultiply されていないのが普通で、現行の「アップロード時に CPU で premul」（§8 冒頭）が使えないため。シェーダの逆premul（`tex.rgb / tex.a`）を**このフラグで飛ばす**だけで済み、パイプラインは増やさない。→ フラグはテクスチャ bind group 側の小さな uniform に置く。**同じ uniform にテクスチャサイズも入れれば、現在 CPU でやっている UV 正規化も GPU へ移せる**（§5.5 と相性が良い）。
- **重複排除は hash マップ側の性質**なので、`SSPack` が GPU を持たなくても「同じキャラ N 体でアップロード1回」は成立する（`SSRenderer` の `hash → entry` が1つだから）。
- **device 喪失復帰の責務分界**（§14）: `upload_texture` 経路（我々が作った）は `SSRenderer` が retained source から復帰させる。`set_texture` 経路（借り物）は**供給側の責務**——engine は自分の asset を作り直したうえで `set_texture` を再度呼ぶ。境界を曖昧にしない。

---

## §9 Rust API（第一級・全インターフェースの根）

**設計原則**: wgpu をイディオマティックに借用する Rust API。`Device`/`Queue`/`Surface`、そして `SSRenderer` を一切所有せず、ホスト（viewer / Bevy / web / ffi）が所有するものを借りて描く。これにより Bevy の render graph へそのまま組み込める。

**借りる対象の一覧**（呼び出し側から見た形）:

```rust
// ホストが1回だけ用意する（device に1個）
let mut renderer = SSRenderer::new(&device);

// 描画ターゲット（canvas / swapchain / offscreen）ごとに1個
let mut scene = SSScene::new(&device, &SceneConfig { color_format, stencil_format, .. });
scene.set_viewport(&queue, w, h, y_down);

// 体数ぶん。GPU リソースを持たないので安い
let mut players: Vec<SSPlayer> = vec![SSPlayer::new(), SSPlayer::new()];

// 毎フレーム
scene.update(&mut renderer, &device, &queue, dt, &mut players);
{
    let mut pass = encoder.begin_render_pass(&desc);   // ホストが開く
    scene.draw(&renderer, &mut pass);
}
```

### 9.1 SSRenderer（層2a・device スコープ）✅ P7-3 実装済（現状 `internal.rs`、7-4 で `renderer.rs` へ）

**device に1個**。フォーマット非依存で、pipeline は使われた組合せだけ遅延生成する（§5.2）。

```rust
pub struct SSRenderer { /* lazy pipelines, layouts, sampler, white fallback, texture cache */ }

impl SSRenderer {
    pub fn new(device: &wgpu::Device) -> Self;                      // ★config を取らない
    pub fn pipeline_count(&self) -> usize;                          // 診断（§5.2）

    // --- テクスチャ ---
    pub fn upload_texture(&mut self, device, queue, hash: u32, rgba: &[u8], size: (u32,u32), retention: Retention);
    pub fn texture_size(&self, hash: u32) -> Option<(f32, f32)>;
    pub fn forget_texture_sources(&mut self);                       // 保持コピーを捨てる（§14）
    // ◻ 7-5: 所有ではなく借用チャネルへ（§8.2）
    // pub fn set_texture(&mut self, hash, view: &wgpu::TextureView, size, alpha: AlphaMode);
    // pub fn forget_texture(&mut self, hash: u32);

    // --- device 喪失復帰（§14）。ここ1回で device 上の全 Scene / Player が復帰する ---
    pub fn restore_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue);
}

/// アップロードしたピクセルを復帰用に保持するか（§14）。
pub enum Retention {
    /// 保持する。ホストが供給し、もう持っていないかもしれないテクスチャ用。
    Retain,
    /// 保持しない。供給元が作り直せるもの——embedded アトラスは Player が
    /// 所有する `.ssab` から再デコードするので、2枚目のコピーは純粋な無駄。
    Reproducible,
}
```

**復帰の責務分界（★7-3 で確定）**: device 喪失時、ホストは **`SSRenderer::restore_device` を1回**呼び、続いて各 Player の `restore_device(&mut renderer, ..)` を呼ぶ。前者が pipeline / テクスチャ（`Retain` 分）を、後者が投影 uniform とフレームアリーナを、そして embedded アトラスの再デコード（`Reproducible` 分）を受け持つ。**旧設計はこの全部を Player ごとに繰り返していた**——アトラスの PNG デコードを含めて。

- パイプラインキャッシュのキーは `(blend, stencil_test, color_format, stencil_format)`。フォーマットをキーに入れることで、**フォーマットの違う複数ターゲットが1つのレンダラを共有できる**。
- 旧 `SSInternalPlayer` の「`new()` で作られ以後不変」な部分がここに来る。

### 9.2 SSScene（層2b・描画ターゲットスコープ）✅ P7-4 実装済（現状 `internal.rs`、ファイル分割は残タスク）

**描画ターゲット1枚に1個**。投影・フレーム頂点アリーナ・draw_call 列・ステンシル管理を持ち、**order 昇順で N 体を1 pass にまとめる**。

```rust
pub struct SSScene { /* projection UB, frame arena (verts/indices/params), draw_calls, stencil state */ }

impl SSScene {
    pub fn new(device: &wgpu::Device, config: &SceneConfig) -> Self;
    pub fn set_viewport(&mut self, queue: &wgpu::Queue, w: f32, h: f32, y_down: bool);
    pub fn resize(&mut self, device, queue, w: u32, h: u32);

    /// 全 Player を進め、1本のアリーナへ積んで GPU へ上げる。
    /// players は order 昇順に整列してから走査（同値は与えられた順を保つ）。
    pub fn update(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue,
                  delta_secs: f32, players: &mut [&mut SSPlayer]);

    /// ホストが開いた RenderPass へ積むだけ。begin/submit/present はホスト責務。
    pub fn draw<'a>(&'a self, renderer: &'a SSRenderer, pass: &mut wgpu::RenderPass<'a>) -> usize;

    pub fn restore_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue);
}
```

- `SceneConfig { color_format, stencil_format: Option<_>, .. }` がターゲット仕様。`stencil_format` が `Some` のときホストは毎 pass に一致する depth/stencil を attach する（§6）。
- **Player は所有せず借りる**（§1.2）。ECS や engine のノードに Player が住むケースをそのまま載せるため。
- 旧 `SSInternalPlayer` の `begin_frame` / `append_*` / `end_frame` / `draw` はここへ移る。`FrameView` は ssruntime の FrameData ＋ SoA スライスをまとめた借用ビュー（`get_frame_data` 直後に構築）、`BatchInfo { start_rank, count, texture_hash, blend_type, kind, stencil }` は Player の木走査が抽出して渡す。
- **draw_call マージ**: アリーナが1本なので、pipeline ＋ texture が一致し index 範囲が連続する draw_call は Player 境界をまたいで結合できる（§1.2）。マスク使用 Player の前後にはステンシル操作が挟まるのでそこは切れる。

### 9.3 SSPlayer（層3・操作）— 幹の公開 API ✅（`libs/ssplayer-wgpu/src/player.rs`）

```rust
pub struct SSPlayer { /* packs: Vec<SSPack>, root: PlaybackNode 木, transform, order, visible, alpha, tint */ }
// ★P7: wgpu 型のフィールドは1つも持たない（§1.2）

impl SSPlayer {
    pub fn new() -> Self;                  // ★device も renderer も取らない。GPU リソースが無いので失敗しない

    // --- pack 登録 / 再生制御 ---
    pub fn add_pack(&mut self, pack: &SSPack);      // 外部参照解決用に全 pack 登録（§8.2）
    pub fn set_pack(&mut self, pack: &SSPack);      // 主 pack 指定
    pub fn set_animation(&mut self, anim: &str) -> Result<(), SSRuntimeError>;  // 選択＝インスタンス木を再構築（★確保はここだけ）
    pub fn play(&mut self);   // 開始 / pause からの再開。引数なし・確保なし
    pub fn pause(&mut self);  pub fn stop(&mut self);  pub fn set_speed(&mut self, speed: f32);
    pub fn is_playing(&self) -> bool;
    pub fn animation_names(&self) -> Option<Vec<String>>;
    pub fn canvas_size(&self) -> Option<(f32, f32)>;

    // --- 配置・合成（★P7 新設。複数体を1シーンに置くために必須）---
    pub fn set_transform(&mut self, x: f32, y: f32, scale_x: f32, scale_y: f32, rotation: f32);
    pub fn set_order(&mut self, order: i32);        // シーン内の描画順。小さいほど先
    pub fn set_visible(&mut self, visible: bool);   // false なら update / draw ごと飛ばす
    pub fn set_alpha(&mut self, alpha: f32);
    pub fn set_tint(&mut self, rgb: u32);
}
```

**借用モデルの要点**: Player は自分では描かない。`SSScene::update` が Player を借りて木を進め、シーンのアリーナへ積む。`begin_render_pass`/`submit`/`present` はホストの責務。

**外部インスタンス解決**: `add_pack` で全 pack を登録 → インスタンスは `ref_anime_pack()` 文字列で resolve（§7.1）。

**再生 API の形（★確定）**: **選択（`set_animation`）と開始（`play`）を分ける**。木・Context・スロットの確保は `set_animation` に閉じ、`play`/`pause`/`stop`/`set_speed` は確保なしの遷移操作にする（ssruntime の `setup_animation` + `play` と同型で、他 Player とも揃う）。`play()` は `pause()` からの**再開**でもある。**★P7 で改定: js 面もこの形に揃える**——旧 `<ss-player>` 互換の `play(name)` 1コールは維持しない（参照実装である ssplayer-webgl 自身が `setAnimation` + `play` に分かれたため。§0.2 / §10）。

**★P7 で移った API**: `upload_texture` は `SSRenderer`（§9.1）、`set_viewport` は `SSScene`（§9.2）、`restore_device` は `SSRenderer`/`SSScene` へ。いずれも「Player が GPU リソースを持たない」の帰結。

**将来 API（webgl 版に追随）**: `box`（追従矩形）/ `clip`（scissor）/ `backdrop`（framebuffer 読み取り系ブレンド用の下地）/ パーツ単位オーバーライド（色・セル・表示）/ イベント（userData・signal・audio・animEnd・animLoop・frame）。§9.5。

> **現状と将来**: エラーは当面 `SSRuntimeError` を流用。将来 `SSPlayerError`（`thiserror`）でラップし FFI 整数コード / JS `JsError` へ変換。ssab の **embedded 自動テクスチャロード**（§8.1）は ✅ 実装済。**seek** は未実装。

### 9.4 各ホストからの駆動（rs / js / ffi 共通の型）

ホストが所有するのは **wgpu 一式 ＋ `SSRenderer` ＋ Player の置き場**。共通ループは `scene.update(&mut renderer, …, &mut players)` → `begin_render_pass` → `scene.draw(&renderer, &mut pass)`。

- **① rs / viewer**: winit で Surface を所有 → 毎フレーム `get_current_texture` → 上記ループ → `submit`/`present`。
- **① rs / Bevy**: Bevy の `RenderDevice`/`RenderQueue`（内部は wgpu）を借りる。`SSRenderer` は Bevy の `Resource`、`SSScene` は描画先ごと、`SSPlayer` は **Component**（ECS に住む）——`SSScene` が Player を所有せず借りる設計（§1.2）はここのため。**テクスチャは Bevy の `Handle<Image>`/`GpuImage` をそのまま借りる**（§8.2）。
- **② js / web**: `web.rs` が canvas から Surface を生成・所有。**device と `SSRenderer` はページで1つ**を共有し、canvas ごとに `SSScene` を持つ（§10）。
- **③ ffi / C・C++**: `host.rs` が raw-window-handle から Surface/Device を生成・所有し、`ss_scene_*` / `ss_player_*` ハンドル API 越しに同じループを回す（§11）。

### 9.5 将来 API（REDESIGN_ASSESSMENT §7）

パーツ単位オーバーライド（色/セル/表示）、ミキシング、ステートマシン。いずれも ssruntime 側の合成で完結し FrameData/DrawBatch 契約を壊さないため、`SSPlayer` に制御メソッドを足すだけで層2 の改修は不要。ssplayer-webgl は既に実装済（`setPartColorOverride` / `setPartCellOverride` / `setPartVisibilityOverride` / `getPartTransform`）なので、移植先の形はそちらに合わせる。

---

## §10 WebGPU / JS 面（② js — Rust API の上のホスト）

- **単一 Rust→WASM バイナリ**（ssruntime + 層2 + player を wasm-bindgen で1つに）。wgpu が WebGPU バックエンドを叩く。
- `libs/ssplayer-wgpu/src/web.rs`（`#[cfg(feature="web")]`）が **canvas から Surface を生成・所有**し、借用モデルの `SSScene`/`SSPlayer`（§9）を駆動する薄いホスト。wasm-bindgen 束縛はこの層。
- **★P7: device と `SSRenderer` はページで1つ**。旧 `SsPlayerWeb` は `create(canvas)` ごとに adapter/device を要求しており、1ページに N 要素置くと **N 個の GPUDevice** ができていた（device をまたぐとテクスチャも pipeline も共有できず、`restore_device` も N 回走る）。新形:

```js
const ctx    = await SsContextWeb.create();      // adapter + device + SSRenderer（ページで1つ・遅延初期化）
const surf   = ctx.createSurface(canvas);        // canvas ごと = SSScene ＋ wgpu::Surface
const player = surf.createPlayer();              // 1 canvas に何体でも
await player.load(url); player.setAnimation('idle'); player.play();
player.setTransform({x, y, scaleX, scaleY, rotation}); player.order = 1;
```

- **JS 露出は再設計後の ssplayer-webgl（`SSSurface` ＋ N 体の `SSPlayer`）と同型**にする（§0.2）。旧 `<ss-player>` の 1 canvas = 1 player・`play(name)` 1コールには揃えない——参照実装から既に消えているため。
- `<ss-player-webgpu>` カスタム要素は「1要素 = 1 canvas = 1 player」の**便利ラッパ**として残す（広告のような単体用途はこれが最短）。ただし内部では上記の共有 device を使い、要素を何個置いても device は1つ。
- ssab は embedded / reference 両対応（§8.1）。**embedded のデコードは `createImageBitmap` ＋ `copy_external_image_to_texture` に寄せ、`image` crate を wasm から外す**（§8——狙いは premultiply の GPU 化とオフスレッドデコード）。
- ビルド/パッケージは `scripts/release-web.sh` 相当を追加し `pkg/ssplayer-wgpu-web/` を吐く。
- 注意: WebGPU 未対応ブラウザへのフォールバック方針（WebGL 版へ退避 or エラー表示）を Web Component 層で定義（§14）。

---

## §11 ネイティブ / C・C++ 面（③ ffi — Rust API の上のホスト）

- `ssplayer-wgpu` の `ffi` feature（`crate-type` は cdylib / rlib / staticlib。crate-type は feature で切り替えられないので常時3種）。`build.rs`＋`cbindgen.toml` で cbindgen → `include/ssplayer_wgpu.h`（feature off なら build.rs は no-op）。
- **層の分担（★厳守）**: `libs/ssplayer-wgpu/src/host.rs` の **`NativeHost`** が生ウィンドウハンドルから Surface/Device を生成・所有し、借用モデルの `SSScene`/`SSPlayer` を駆動する（§9.4）。`src/ffi.rs` は **ABI 変換だけ**——null/UTF-8 検査、`*const u8`→スライス、エラー→整数コード——を行い、実処理は `NativeHost` / `SSPlayer` に委ねる。**ffi.rs にロジックを書かない**（書きたくなったら置き場所は host.rs か player.rs）。
- **★P7: シーンとプレイヤーを別ハンドルに分ける。** 旧 API は `ss_player_t` がホスト（Surface/Device/swapchain）と Player を融合しており、**1ウィンドウに2体を置くことが機能として成立しなかった**——`ss_player_create` が生ウィンドウハンドルを取るので、2体目は同じウィンドウに2枚目の Surface を作り、双方が `clear`＋`present` して消し合う。web の「device が N 個できて重い」より一段悪い。
- C からはハンドルベース API で操作:

```c
typedef struct ss_scene_t  ss_scene_t;                        // 不透明ハンドル（ウィンドウ + device + renderer + scene）
typedef struct ss_player_t ss_player_t;                       // 不透明ハンドル（再生木。scene が所有）

ss_scene_t*  ss_scene_create(const ss_scene_desc_t* desc);    // 生ウィンドウハンドル + backend + size + clear + mask
void         ss_scene_destroy(ss_scene_t*);                   // 配下の player もまとめて破棄
void         ss_scene_update(ss_scene_t*, float delta_seconds);
int32_t      ss_scene_render(ss_scene_t*);                    // swapchain 取得 → 全 player 描画 → present
void         ss_scene_resize(ss_scene_t*, uint32_t w, uint32_t h);  // ★ピクセル単位
int32_t      ss_scene_add_texture(ss_scene_t*, uint32_t hash, const uint8_t* rgba, uint32_t w, uint32_t h);  // reference 用（renderer 共有）

ss_player_t* ss_scene_create_player(ss_scene_t*);             // ★1シーンに何体でも
void         ss_player_destroy(ss_player_t*);                 // シーンから外して破棄
int32_t      ss_player_add_ssab(ss_player_t*, const char* pack, const uint8_t* data, size_t len);   // 外部参照解決用に全 pack 登録
int32_t      ss_player_load_ssab(ss_player_t*, const char* pack, const uint8_t* data, size_t len);  // 主 pack。embedded/reference 両対応
int32_t      ss_player_set_animation(ss_player_t*, const char* anim_name);   // 選択（確保はここだけ）
void         ss_player_play(ss_player_t*);                    // 開始 / pause からの再開
void         ss_player_pause(ss_player_t*);  void ss_player_stop(ss_player_t*);
void         ss_player_set_speed(ss_player_t*, float speed);  bool ss_player_is_playing(ss_player_t*);
void         ss_player_set_transform(ss_player_t*, float x, float y, float sx, float sy, float rot);
void         ss_player_set_order(ss_player_t*, int32_t order);       // 小さいほど先に描く
void         ss_player_set_visible(ss_player_t*, bool visible);
uint32_t     ss_player_animation_count(ss_player_t*);
const char*  ss_player_animation_name(ss_player_t*, uint32_t index);  // 次の呼び出しまで有効
int32_t      ss_player_canvas_size(ss_player_t*, float* w, float* h);
const char*  ss_last_error(void);                             // スレッドローカル。次の失敗まで有効
```

- **ハンドルの寿命**: `ss_player_t*` は `ss_scene_t` が所有する要素への参照。**シーンを破棄したら配下の player ハンドルは全て無効**（use-after-free になるので C 側で持ち越さない）。世代付きインデックスで dangling を検出できるようにする。

- **ステータス規約**: `SS_OK`(0) ／ **正 = ssruntime のエラーコード**（`SSRuntimeError::as_code`、SDK の C-API と同番号）／ **負 = Player/ホスト側のエラー**（`SS_ERROR_INVALID_ARGUMENT` = -1、`SS_ERROR_GPU` = -2）。詳細メッセージは `ss_last_error()`。
- **ヘッダは C。C++ からもそのまま使える**（`cpp_compat = true` で `extern "C"` ガード付き）。別途 `.hpp` は生成しない——cbindgen の C++ 出力は同じ平坦 API の別表記で得がなく、欲しいのは RAII ラッパのような**手書きの薄い C++ 層**であり、必要になった時点で追加する。
- **アンワインドしない**: ワークスペースは `panic = "abort"` なので、パニックが ABI 境界を越えることはない。
- **ウィンドウハンドルの渡し方**（§14）: プラットフォーム別の生ハンドルを平坦な `ss_window_handle_t{ kind, screen, window, display }` に詰めて渡し、`ffi.rs` が `raw-window-handle` 型（または CAMetalLayer 用の専用ターゲット）へ組み立てる。`kind` は `SS_WINDOW_WIN32 / APPKIT / METAL_LAYER / XLIB / WAYLAND`。

- **自己完結ホスト（既定）**: `ss_scene_desc_t` に**生ウィンドウハンドル**（Windows: HWND+HINSTANCE / macOS: NSView or CAMetalLayer / Linux: Xlib/Wayland）＋ backend ヒント（DX12/Vulkan/Metal/自動）＋ サイズ・クリア色・`enable_mask`（stencil 確保）・`y_down` を渡し、`host.rs` が Surface/Device を生成。per-backend 確認 viewer が各バックエンドを強制指定して検証。
- **既存エンジンのデバイス相乗りは C-API の対象外（★P7 で確定した割り切り）**: wgpu の `Device`/`Queue`/`TextureView` は Rust 型であり C ABI に載らない。wgpu-hal 相互運用で無理に通しても、呼び出し側は結局 wgpu-native と ABI とバージョンを合わせる必要があり、C ホストにとって嬉しくない。**engine 組み込みは Rust 専用能力**とし、アダプタは `libs/` 配下の別クレートで提供する（§2 のクレート方針と一致）。C から engine に載せたい場合の現実解は**オフスクリーン描画＋共有テクスチャ**（Metal shared texture / D3D shared handle）で、これは別フェーズ扱い。
- パッケージは `release-macos.sh` / `release-windows.ps1` / `release-linux.sh` 相当で cdylib/staticlib + ヘッダを配布。

---

## §12 実装フェーズ（P0〜P6 完了 → **現在 P7**）

| Phase | 内容 | 完了条件 |
|---|---|---|
| **P0（初手）3層リファクタ＋借用モデル Rust API** ✅ | 現 `core/`（Player/Renderer）を単一クレート内の `internal`(SSInternalPlayer) / `player`(SSPlayer) モジュールへ再編。**SSPlayer を借用モデル（`&Device`/`&Queue` を借りる §9）に確定**。データ所有契約（§1.1）を配線。viewer が wgpu 一式を所有し SSPlayer を駆動 | 現状の描画（Normal/Mesh/4blend）が3層＋借用モデルで従来通り動く。**これが ① rs 面の土台**（完了） |
| **P1 ssab 両対応 + WebGPU（② js）** ✅ | **ssab embedded/reference 両ロード（§8.1）** ＋ **WebGPU/js**：`ssplayer-wgpu` の `web` feature（wasm-bindgen、`libs/ssplayer-wgpu/src/web.rs` の `SsPlayerWeb` が canvas から wgpu を所有し借用モデル SSPlayer を駆動）、`examples/web` の `<ss-player-webgpu>` カスタム要素＋build.sh。別 crate は作らず単一クレート。**wasm ビルド要件**: `web` feature が `ssruntime/std` を有効化（cargo は依存の全 crate-type を build するため、ssruntime の no_std cdylib/staticlib が wasm でアロケータ/パニック不足になる。std ならその衝突が消える。`ssruntime/panic-handler` は player の std と `panic_impl` 重複で不可）。release wasm-opt 用に `[package.metadata.wasm-pack.profile.release]` の SIMD 等フラグを player crate にも付与 | **ブラウザ実機で WebGPU 描画確認済**（release wasm 537KB） |
| **P2 描画パリティ①** ✅ | varg 追加（PartColor blend 由来）、BlendType 全12種のパイプラインマトリクス、premul 厳密化（upload premul ＋ 逆/再premul シェーダ ＋ premul ブレンド係数） | PartsColor / BlendMode / Basic / Effect を PNG 目視で確認。既存フィクスチャのピクセル数は不変。WebGL 版と色・ブレンド一致 |
| **P3 マスク** ✅ | stencil 付きターゲット（config opt-in）、MaskPlanner 移植（`libs/ssplayer-wgpu/src/mask.rs`）、write/test パイプライン、pure マスク色スキップ、インスタンス継承、**Shape 幾何のマスク writer ＋ Shape content 描画**、白フォールバック初期化（§6） | マスク/Shape アニメが描画（Mask.ssab / Shape.ssab をヘッドレス GPU readback ＋ 実機で確認） |
| **P4 エフェクト/インスタンス** ✅ | **P0 の次に先行実施**。①内部を `begin_frame`/`append_batch`/`append_effect`/`end_frame` の累積 API＋transform へ（Stage 1）②インスタンス木＝子ごと独立 Context（Stage 2, §7.1）③エフェクト＝EffectSlot/EffectDrawPlan（Stage 3, §7.2） | エフェクト/インスタンスが描画される（実機確認済） |
| **P5 ネイティブ C-API + C++ viewer（③ ffi）** ✅（Metal） | `ffi` feature ＋ cbindgen（`build.rs`/`cbindgen.toml` → `include/ssplayer_wgpu.h`）、`host.rs` の `NativeHost`（生ハンドル→wgpu 所有）＋ `ffi.rs`（ABI 変換のみ）、`examples/viewer-metal-cpp`（Cocoa/Objective-C++、CMake が cargo を駆動）。再生 API は `set_animation` + 引数なし `play`（§9.3）。◻ 残: `viewer-dx12-cpp`（Windows）・`viewer-vulkan-cpp`（Linux）／P7 で `ss_scene_*` へ作り直し | **macOS 実機で Metal 描画確認済**（Basic / Mask を C-API 経由で描画）。DX12/Vulkan は各実機で要確認 |
| **P6 統合シェーダ13種** ✅ | ss_shader 全13種（sepia/hsb/step/pix/noise/bmask/outline/blur/wave/move/scatter/circle/spot）を WGSL 移植（`internal.rs` `SS_SHADER_WGSL`）。パーツ属性 SHDR を per-part ディスパッチ：`(id_hash, blend)` 毎に**遅延生成**するシェーダパイプライン＋group(2) の動的オフセット uniform（param0..7 / cell_rect / pixel_size）。各シェーダは straight-alpha の Pixel を計算し `ss_finish`（＝P2 の PartsColor tail）で合成（blur のみ例外）。`ssTex` は逆premul＋`textureSampleLevel`（ループ/discard 後のサンプル可）。**制限（WebGL v1 同）**: map0/map1 未対応・shader×マスク非結合・Normal パーツのみ | Shader.ssab をヘッドレス GPU＋PNG 目視（13種の効果を確認）。shader 無しアニメはコスト0 |
| **P7 シーン再設計（複数プレイヤー）** 🚧 **進行中**（7-1 / 7-2 / 7-3 / 7-4 ✅） | 下表の 7-1〜7-8。`SSInternalPlayer` を `SSRenderer`(device) / `SSScene`(ターゲット) へ解体し、`SSPlayer` から GPU リソースを剥がす（§1.2）。**破壊的変更・後方互換は取らない**（プロト段階） | 1つの描画ターゲットに複数体を正しく描ける（マスク持ち同士を含む）。web は device 1個で N 要素。C は 1 window に N 体 |
| （任意）Bevy アダプタ | `ssplayer-wgpu-bevy`（借用モデルの一利用例）。コアに依存を入れず別立て。**P7 の `set_texture` 借用チャネル（§8.2）が前提** | Bevy の render graph で `SSScene` が描画。テクスチャは `GpuImage` を借用（二重アップロードなし） |
| （後回し）Text / Nines | 旧版も未実装 | 必要時 |

### 12.1 P7 の分解と依存関係

```
[7-1] ステンシル完全巻き戻し ──┐   ← 単独で入る。何をするにも先に
[7-2] transform/order/visible ─┤   ← 単独で入る。配管は既存（§5.4）
[7-3] SSInternalPlayer 解体 → SSRenderer(device) + フレーム蓄積
                               ↓
[7-4] SSScene（アリーナ / projection / order 合成 / restore の単位）
                               ↓
        ┌──────────────────────┼──────────────────────┐
[7-5] pack 共有 + set_texture   [7-6] SoA パススルー    [7-7] C-API 作り直し
      借用チャネル（§8.2）            （§5.5）              （ss_scene_*）
                                       ↓
                              [7-8] 静的/indexless index buffer
                                       ↓
                              render bundle / timestamp query
```

| # | 内容 | 参照 | 単独で入るか |
|---|---|---|---|
| 7-1 ✅ **実装済** | ステンシル完全巻き戻し（不変条件）。`player.rs` の `unwind_masks` | §6 | 単独で入った |
| 7-2 ✅ **実装済** | `set_transform` / `reset_transform` / `set_order` / `set_visible` / `set_alpha`。`order` は保持のみ（消費するのは 7-4 の `SSScene`）。`tint` は §5.5 後に回す——PartsColor の Mix 式より後段に掛ける必要があり、頂点色への畳み込みでは表現できない | §5.4 / §9.3 | 単独で入った |
| 7-3 ✅ **実装済** | パイプライン遅延生成（39本 eager → 使ったぶんだけ）＋ `SSRenderer` 分離。ホストが所有し `update`/`draw` で貸す。テクスチャ・pipeline・layout・sampler・白フォールバックが device 単位の共有に。**ファイルはまだ `internal.rs`**——`renderer.rs` / `scene.rs` への分割は 7-4 と同時に1回で行う（2度のファイル移動を避けるため） | §5.2 / §9.1 | 遅延生成は単独で先行 |
| 7-4 ✅ **実装済** | `SSScene`（旧 `SSInternalPlayer` を改名）。1 シーンが N プレイヤーを `order` 昇順で1フレームに積む。`SSPlayer` から **GPU 型のフィールドが完全に消えた**（`new()` は引数なし）。投影・アリーナ・復帰はシーン側へ。◻ 残: ファイル分割（`renderer.rs` / `scene.rs`） | §9.2 | 7-3 の後 |
| 7-5 | `SSPack` ＋ `set_texture` 借用チャネル | §8.2 | 7-3 の後 |
| 7-6 | SoA パススルー（GPU 頂点展開） | §5.5 | 7-4 の後 |
| 7-7 | C-API を `ss_scene_*` + `ss_player_*` へ | §11 | 7-4 の後 |
| 7-8 | 静的/indexless index buffer、draw_call マージ、マスクのランレングス結合 | §5.5 / §6 | 7-6 の後（マージ・ランレングスは単独可） |

**単独で先に入る小改善**（P7 を待たない）: 7-1 / 7-2 / パイプライン遅延生成 / マスク時のランレングス結合 / `position.z` 削除（§5.1） / premul スクラッチ再利用（§8）。

---

## §13 検証

| 対象 | 手段 |
|---|---|
| Rust 描画（開発） | `examples/viewer/`（winit+wgpu、`assets/overall/` 再生。横=ssab切替/縦=アニメ切替） |
| インスタンス/エフェクト（ヘッドレス） | `libs/ssplayer-wgpu/tests/instance.rs`（probe。frame 推移・quad 生成を GUI 抜きで確認） |
| マスク/Shape（ヘッドレス） | `libs/ssplayer-wgpu/tests/mask.rs`（planner probe：writer 分類・Shape-mask）＋ `libs/ssplayer-wgpu/tests/render.rs`（GPU offscreen readback：masking on/off 差分・Shape content 描画量。adapter 無ければ skip。PNG 保存で目視も可） |
| 統合シェーダ（ヘッドレス） | `libs/ssplayer-wgpu/tests/render.rs::render_integration_shaders`（Shader.ssab を描画し、SS WGSL コンパイル＋per-part ディスパッチ＋params で描画量が出ることを確認。adapter 無ければ skip） |
| WebGPU（主） | `examples/web/`（ブラウザで `<ss-player-webgpu>`）。初回起動計測（取得→wasm初期化→初フレーム ms） |
| C-API（③ ffi） | `cargo build -p ssplayer-wgpu --features ffi`（cbindgen がヘッダを再生成。**ネイティブの他コマンドではこの面はコンパイルされない**——web feature と同じ死角） |
| DX12（副） | `examples/viewer-dx12-cpp/`（Windows 実機、backend=DX12 強制） |
| Metal（副） | `examples/viewer-metal-cpp/`（macOS 実機、backend=Metal 強制）。`cmake -B build && cmake --build build && ./build/ssviewer-metal [asset-dir] [pack]` |
| Vulkan（将来） | `examples/viewer-vulkan-cpp/`（Linux 実機、backend=Vulkan 強制） |
| **複数プレイヤー（ヘッドレス）** ✅ P7-1/7-2/7-3 | `libs/ssplayer-wgpu/tests/scene.rs`。**レンダラ共有**は `players_share_one_renderer`（同一 pack の2体目でパイプライン本数が増えないこと）と `pipelines_are_built_on_demand`（構築直後は0本、Basic.ssab で1本、同一フレーム再構築で増えない＝キーが安定）で見る。**ステンシル残留の回帰**は「1 pass に N 体」と「1 体 1 pass（stencil を毎回クリア）」の**ピクセル完全一致**で検出する——後者は構造的に漏れようがないので基準になる。マスク持ち同士（`Mask.ssab` ×2）と混在（`Mask` × `Basic`、順序両方向）。`set_transform` は**同一フレームで**（`update(0.0)` で再構築）整数ピクセル平行移動し、元画像を N px ずらしたものと完全一致することを要求。`set_visible(false)` は描画ゼロ、`set_alpha(0.0)` も描画ゼロ、0.5 はその中間。◻ 残: `order` の並べ替え、同一 pack N 体のアップロード1回（どちらも P7-4/7-5 で `SSScene`/`SSRenderer` ができてから） |
| **複数プレイヤー（目視）** | ◻ `examples/viewer` を N 体対応にする。現状 1 体固定で、**複数体でのパリティ検証ができない**（マスク・ブレンドの相互干渉は目視でしか気づけない類） |
| **wasm サイズ** | `cd examples/web && ./build.sh`（**release**。`--dev` は wasm-opt を飛ばすので測ってはいけない）→ `ls -l pkg/*.wasm pkg/*.js` ＋ `gzip -9 -c ... \| wc -c`。§14.1 の表を更新する |
| WebGL 版との一致 | 同一 ssab を両者で再生し目視/スクリーンショット比較（色・ブレンド・マスク）。**P7 以降は「複数体を置いた画」でも比較する**（ssplayer-webgl も surface+players 構成のため同条件） |
| Rust 品質 | `cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --all -- --check` / `cargo test` |

---

## §14 既知の制約・要調査

- **premul の一貫性**が最重要。アップロード・シェーダ・ブレンド係数のどこか1つでも崩すと合成破綻。
- **premul と WebGPU の非対称（★要注意）**: WebGL は `UNPACK_PREMULTIPLY_ALPHA_WEBGL` でドライバが無料で premultiply するが、**WebGPU の `writeTexture` に等価は無い**。現行は CPU で premul しており、2048² で 16 MB の一時コピーが出る。web は `copy_external_image_to_texture` ＋ `createImageBitmap` へ寄せる（§8）。engine から借りるテクスチャは premul されていないのが普通なので `AlphaMode` フラグで受ける（§8.2）。
- **stencil ターゲット** ✅ 実装済: `SSPlayerConfig.stencil_format`（P7 以降は `SceneConfig`）で opt-in し、ホストが毎 pass に attach（§6）。
- **INCR/INVERT 混在マスク**は単一ステンシルで近似（厳密解は coverage-bitmap、要検討）。
- **ステンシル残留**（P7 で解消予定）: `render_node` がフレーム末尾で未消化 writer を巻き戻さないため、**1 pass に複数 Player を積むと A のマスクが B をクリップする**。現状は 1 pass = 1 player ＋ `LoadOp::Clear(0)` で隠れているだけ（§6）。
- **WebGPU 非対応環境のフォールバック**方針（WebGL 退避か）未定 → §14.1。
- **raw-window-handle の FFI 越え** ✅ 実装済: Rust トレイトは C に渡せないため、平坦な `ss_window_handle_t{ kind, screen, window, display }` を C-API に定義し、`ffi.rs` が `raw-window-handle` 型へ組み立てる（§11）。CAMetalLayer だけは rwh に表現が無いので wgpu の `SurfaceTargetUnsafe::CoreAnimationLayer` を使う（Apple ターゲットのみ）。**Win32/Xlib/Wayland の経路は実機未検証**——DX12/Vulkan viewer で確認する。
- **テクスチャ供給**: embedded（Player がデコード）/ reference（ホスト供給）の両対応（§8.1）。埋め込みテクスチャ列挙 API の正確な形は `libs/ssab/SCHEMA.md` / `SsabResource` で要確認。
- **データ所有契約（§1.1）**: ssab は Player 所有→ssruntime へ borrow、FrameData/SoA は ssruntime 所有→Player が borrow。ssab を Context より先に破棄しない／FrameData 借用をフレーム跨ぎで保持しない。
- **エフェクト blend はコマンド毎**（batch.blend_type ではない）点を実装で取り違えない。
- **ss_shader 統合シェーダ13種** ✅ 実装済（`internal.rs` `SS_SHADER_WGSL`、WebGL `SSShaderLib.ts` を WGSL 移植）。残課題: map0/map1 セカンダリテクスチャ・shader×マスク結合・GLSL 版との厳密ピクセル一致検証。
- **WebGL 版からの堅牢性 carry-over（借用モデル前提）** ✅ 実装済: WebGL 版 Player（`SSPlayerForWeb`）で実装済みの広告向け堅牢性を wgpu へ持ち込む対応表。Device/Queue/Surface は**ホスト所有**（§1.1 借用モデル）なので、喪失検知・再生成が**ホスト責務**になる点が WebGL 版との最大の差。
  - **デバイス/サーフェス喪失復帰** ✅ 実装済: **`SSPlayer::restore_device(&Device, &Queue)`**（`player.rs`）が旧デバイス向けの pipeline / buffer / bind group / texture / 投影行列を新デバイスへ全再生成する（WebGL 版 `SSWebGLRenderer.restoreContext()` ＋ `SSWebGLResourceManager.restoreContext()` 相当）。再生状態（ロード済みパック・インスタンス木・現在フレーム）は CPU 側なので無傷——**中断したフレームから再開**する。テクスチャは**生成元を保持して再アップロード**（retained-source）: embedded は Player が所有する `.ssab` から**再デコード**するのでピクセル複製を持たず、ホスト供給分のみ `upload_texture` の straight RGBA を保持する（複製を嫌うホストは `forget_texture_sources()` で破棄し自前で再供給）。**カラー/ステンシルのフォーマットが変わった場合は対象外**——SSPlayer を作り直すこと。
    - web(§10): `SsPlayerWeb` が `Device::set_device_lost_callback`（wgpu の webgpu バックエンドが `GPUDevice.lost` を購読）で喪失を検知し、`isDeviceLost()` で公開。`restoreDevice()`（async）が adapter/device を再取得 → surface / stencil 再構成 → `SSPlayer::restore_device`。喪失中の `frame()` / `resize()` は GPU に触れない（`resize` はサイズのみ記録し、復帰時に反映）。
    - native(§11): `SurfaceError::Lost/Outdated` は `NativeHost::render` が**サーフェス再構成**（resize と同経路）で吸収済み。全デバイス喪失は稀なので `NativeHost` 側の再生成経路は未実装（必要になれば `SSPlayer::restore_device` を呼ぶだけ）。
    - 検証: `tests/render.rs::restore_device_rebuilds_gpu_state`（**2枚目の Device** へ復帰させ、無中断レンダーとピクセル一致を要求。`forget_texture_sources` 後は不一致になることを negative control として確認）。
  - **可視時のみ描画（pauseWhenHidden・web のみ）** ✅ 実装済: `<ss-player-webgpu>`（`examples/web/ss-player-webgpu.js`）が **IntersectionObserver ＋ visibilitychange** で画面外/裏タブ時に rAF ループを止める（WebGL 版 `SSSurface` の `_shouldRun` / `_syncTicker` と同構造。再開時は時計をリセットして待機時間ぶんの巨大 delta を避ける）。既定 ON、`pause-when-hidden="false"` で明示 opt-out。IntersectionObserver の初回コールバックまでは**非表示扱い**——画面外の初期フレームを描かない。native は winit の `Occluded`/`RedrawRequested` が相当。
  - **精度指定は非該当**: WGSL は f32 既定で `mediump/highp` の概念が無いため、WebGL 版の fragment highp 検出はポート不要。
  - **プレゼンテーション設定**: web は `context.configure({ alphaMode:'premultiplied', … })` ＋ `requestAdapter({ powerPreference:'low-power' })` が WebGL の `premultipliedAlpha` / `low-power` 相当。`desynchronized` / `failIfMajorPerformanceCaveat` に WebGPU 直接等価は無い。

### 14.1 WebGPU を選ぶ理由と、ssplayer-webgl を残す理由

**両者は置き換え関係ではない。** 棲み分けを明文化しておく（「WebGL 版はいずれ畳む」という前提を持たないこと）。

**WebGPU でしかできないこと（＝この Player の存在理由）**

| 能力 | WebGL 側 | この Player で何が変わるか |
|---|---|---|
| **compute / storage buffer** | WebGL2 に SSBO も compute も無い（**原理的に不可**） | SoA パススルー（§5.5）。CPU の頂点組み立てが消え、体数・パーツ数の天井が上がる |
| **同一 device を engine と共有** | GL は global state の奪い合いで実質不可能 | **本命**。Slint / Bevy と1 device で合成できる（借用モデルはこのためにある） |
| **native と同一コード** | web 専用 | Metal / Vulkan / DX12（`examples/viewer-metal-cpp` 実績あり） |
| render bundle / indirect draw / timestamp query | 無し（`EXT_disjoint_timer_query` は Spectre 対策で実質消滅） | draw_call 再 encode の削減、GPU 側カリング、バッチ単位の実測 |
| 安いオフスクリーン＋リードバック | `readPixels` は同期ストール | group alpha の厳密実装、テストのピクセル比較、サムネイル生成 |

**それでも WebGL を選ぶ理由（弱み）**

1. **到達率。これが決定打であり、事実上これだけ。** Safari は 26（2025）、Firefox は 141（Windows・2025）でようやく。**iOS 26 未満には無い**。広告クリエイティブ・教材のような「全端末で必ず動く」用途は WebGL2/1 一択（正確な数字は caniuse で都度確認）。
2. **初期表示レイテンシ。** adapter → device が2段の Promise、加えて現行はパイプライン39本を起動時に全コンパイル（→ P7 で遅延生成、§5.2）。WebGL 版は program 1〜2本を遅延コンパイル。First Paint が KPI の用途では効く。
3. **フォールバックの二重保守。** 非対応なら別レンダラが要る＝ WebGL 版は結局残る。
4. **canvas 周りの機能差**（`desynchronized` / `failIfMajorPerformanceCaveat` / `preserveDrawingBuffer` に等価なし）、**premul の非対称**（§8）、**ツールの成熟度**（Spector.js 相当が弱い）。

**バイナリサイズは弱点ではない（★2026-07 実測で訂正）**

|  | ssplayer-wgpu (WebGPU) | ssplayer-webgl | 比 |
|---|---|---|---|
| wasm | 577,338 B | 277,601 B（`ssruntime_bg.wasm`） | |
| JS | 65,146 B（wasm-bindgen glue） | 301,707 B（`ss-player.element.min.js`） | |
| **raw 合計** | **642,484 B** | **579,308 B** | **1.11×** |
| **gzip 合計** | **242,517 B** | **147,415 B** | **1.65×** |

wgpu の WebGPU バックエンドは WGSL をブラウザへそのまま渡すので **naga のバックエンド群が丸ごと落ちる**のが効いている。`image` を外せば（§8）さらに縮む。

> **注意（この訂正の経緯）**: 当初「約10倍・3.8 MB」と書いていたが、それは `build.sh --dev` の成果物だった（wasm-opt 未適用、`name` カスタムセクションだけで 1.76 MB）。**サイズを語るときは必ず release ビルドを測ること。** 計測コマンドは §13。

**方針**: WebGPU 面は「表現力と native 統合」を取りに行く。**到達率**が要件の案件は ssplayer-webgl を使う——サイズは選択理由にならない。`<ss-player-webgpu>` は非対応ブラウザで**エラー表示ではなく WebGL 版へ退避できる形**を目標にする（実装方針は未定）。

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
- 概要/残タスク: `../SSPlayerForWeb/packages/ssplayer-webgl/ROADMAP.md`（旧 `SSPLAYER_WEBGL_SPEC.md` は廃止 → ROADMAP 化。設計詳細はソースコメントへ集約）
- **★シーン構成（P7 が揃える相手）**: `.../src/SSSurface.ts`（1 context に players[]、owned/borrowed/overlay の3モード、order ソート、共有テクスチャキャッシュ、一括 restore）・`.../src/SSPlayer.ts`（transform / box / order / visible / alpha / tint / clip / backdrop / パーツオーバーライド / イベント）
- レンダラ: `.../src/SSWebGLRenderer.ts`（＝我々の `SSRenderer` に対応。GL の program は context 単位だが wgpu の pipeline は device 単位なので、共有範囲は我々の方が広い）
- シェーダ: `.../src/programs/PartsColorProgram.ts` / `MaskProgram.ts` / `SSShaderLib.ts`
- テクスチャ: `.../src/SSWebGLTextureCache.ts`（surface 共有キャッシュ）・`.../src/SSWebGLResourceManager.ts`（player 側）
- 堅牢性（ロスト復帰/可視制御/精度/属性）: `.../src/SSSurface.ts`・`.../src/SSWebGLResourceManager.ts`・`.../src/programs/PartsColorProgram.ts`
- コールドスタート雛形: `../SSPlayerForWeb/TYRANO_COLDSTART.md`

**現行 wgpu 実装（P7 の改修対象）**
- `libs/ssplayer-wgpu/src/internal.rs`（層2。P7 で `renderer.rs` / `scene.rs` へ解体）
- `libs/ssplayer-wgpu/src/player.rs`（再生木。P7 で GPU リソースを剥がす）
- `libs/ssplayer-wgpu/src/web.rs`・`host.rs`・`ffi.rs`（各ホスト。P7 で 1 ターゲット N 体へ）
- `examples/viewer/src/main.rs`（winit+wgpu ループ。P7 で N 体対応）

**全体方針**
- `REDESIGN_ASSESSMENT.ja.md`（頭脳/体、オーバーライド/ミキシング/ステートマシンの将来計画）
