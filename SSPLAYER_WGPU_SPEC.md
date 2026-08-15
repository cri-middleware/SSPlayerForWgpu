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
- **進捗**: **P0**（3層リファクタ＋借用モデル）✅ ／ **P4**（インスタンス §7.1・エフェクト §7.2）✅ ／ **P3**（マスク・Shape）✅ ／ **P2**（色パリティ: varg・全12ブレンド・premul 厳密化）✅ ／ **P6**（統合シェーダ ss_shader 全13種）✅ ／ **P1**（ssab embedded/reference 両対応・**WebGPU/js**）✅ ブラウザ実機確認済 ／ **P5**（ネイティブ ffi・C-API ＋ Metal C++ viewer）✅ macOS 実機確認済、DX12 viewer は実装済・実機未検証、Vulkan viewer は未着手 ／ **P7**（シーン再設計：SSRenderer/SSScene 分離・複数プレイヤー・SoA パススルー）✅ **完了**——**7-1〜7-7・7-9 ✅**（1 ターゲットに N プレイヤー／pack 共有・借用テクスチャ／**頂点バッファ撤去**／C-API 2ハンドル化／**web 面 3オブジェクト化・device はページで1つ**）。**7-8 のみ計測により保留**（下記）。対応バッチ = Normal / Mesh / Shape / Instance / Effect / Mask。**wgpu 29**（§2.2）。詳細は §12。
- **7-8（draw マージ・マスクのランレングス結合）は保留**: 上限は `SSScene::stats()` の `mergeable_draws` が数えており、256体で 2496→1728 draws＝**削れるのは encode 0.09 ms の 31%≒0.03 ms**（予算 16.6 ms の 0.2%）。§14.2 が同じ理由で項目 7 / 4 を見送ったのと同根で、しかも上限はそれらより小さい。**やるならステンシル境界と `shader_slot` を跨がない条件付き実装**になるため、割に合わない（§14.2 結論）。
- **次の大物**: **表現力側**——§14.2 の **1（マスクの coverage-bitmap 厳密化）と 2（背景読みブレンド `Div2`/`Overlay2`）**。速度側 6 項目（4〜9）は計測の結果「着手する理由が無い」と結論済み（256体で CPU 1.3〜2.3 ms ＋ GPU 0.19 ms）。**項目6（プレイヤー走査のスレッド化）は実装したうえで撤去**——native 専用パスを web が一度もコンパイルしないコストに、256体で 0.53 ms は見合わない（§14.2）。**web だけは未計測**だが、計測器は入った——`surface.stats()`（§10）。
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
| 復帰用のテクスチャ元データ | **供給元** | `upload_texture` 経路のみ `SSRenderer`。`set_texture`（借り物）は供給側の責務 | CPU 側にも 16MB／2048²（アトラス1枚につき1本。**premul 済みバッファをそのまま保持**するので、旧設計の「retain コピー＋premul 一時コピー」の2本は消えた——§14） |
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
│   │       ├── renderer.rs        ✅ 層2a SSRenderer（device スコープ。pipeline 遅延生成 ＋
│   │       │                         layout/sampler/白 ＋ テクスチャ ＋ WGSL・頂点形式）§9.1
│   │       ├── scene.rs           ✅ 層2b SSScene（ターゲットスコープ。投影 ＋ 頂点アリーナ ＋
│   │       │                         draw_call ＋ order 昇順で N プレイヤーを1フレームに）§9.2
│   │       ├── pack.rs            ✅ SSPack（.ssab の共有単位。★CPU のみ・GPU 非所有）§8.2
│   │       ├── player.rs          ✅ 層3 SSPlayer（操作。再生木 ＋ transform/order/visible）
│   │       │                         ★P7 以降 wgpu 型のフィールドを持たない（§1.2）
│   │       ├── mask.rs            ✅ MaskPlanner（§6）
│   │       ├── roster.rs          ✅ Roster<T>（世代付き slot）。ハンドルを配る2面が共有 §11
│   │       ├── web.rs             ✅ #[cfg(feature="web")] wasm-bindgen（② js）
│   │       │                         ✅ P7-9: SsContextWeb(ページ) / SsSurfaceWeb(canvas) /
│   │       │                            SsPlayerWeb(体)。device と SSRenderer はページで1つ §10
│   │       ├── host.rs            ✅ #[cfg(feature="ffi")] NativeHost（生ハンドル→wgpu 一式を所有）
│   │       │                         ✅ 1 window = N player（roster.rs の世代付き slot）
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
    ├── web/                       ✅ WebGPU ブラウザ確認（② js）
    │   ├── ss-context.js          ページで1つの SsContextWeb（＋喪失復帰の集約）§10
    │   ├── ss-player-webgpu.js    <ss-player-webgpu>：1要素=1canvas=1体 の便利ラッパ
    │   └── index.html             ①要素 と ②1 canvas に N 体（stats 表示）の2デモ
    ├── viewer-dx12-cpp/           △ C++ DX12 確認 viewer（Win32 HWND → C-API → backend=DX12）**実機未検証**
    │   ├── CMakeLists.txt         cargo build --features ffi を叩き、DLL＋インポートライブラリ＋生成ヘッダを消費
    │   └── src/main.cpp           Win32。HWND を所有し C-API を駆動。.ssab の D&D 対応（metal 版の対訳）
    ├── viewer-metal-cpp/          ✅ C++ Metal 確認 viewer（Cocoa CAMetalLayer → C-API → Metal）
    │   ├── CMakeLists.txt         cargo build --features ffi を叩き、cdylib＋生成ヘッダを消費
    │   └── src/main.mm            Objective-C++。NSWindow/CAMetalLayer を所有し C-API を駆動。.ssab の D&D 対応
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
**✅ 全12種に対応（P7-3 以降は使ったものだけ遅延生成）**（`renderer.rs` `BLEND_ORDER` / `blend_state`、premul 係数）。GPU ネイティブ等価のない `Div2/Overlay2` は Mix、`Mulalpha/Mul2` は Mul、`Screen2` は Screen へフォールバック（ssplayer-pixi 準拠、要検証分は SS6PlayerForUnity と照合予定）。stencil 有効時は各ブレンド × 3（None/Inside/Outside）。

---

## §5 層2（描画）の設計

### 5.1 頂点の受け渡し — **頂点バッファは無い**（★P7-6 で撤去）✅

**現在の実装には `VertexBufferLayout` も index buffer も存在しない。** 頂点属性は §5.5 の SoA パススルーに置き換わり、頂点ステージが scene のストレージバッファから直接読む。以下は「何が頂点として届くか」の意味論であり、**バッファの中身ではない**（`VertexOutput` の中身である）。

| varying | 内容 |
|---|---|
| clip_position | 投影後。z は `0.5` 直書き（下記） |
| uv | 正規化 UV（0..1）。**正規化は頂点ステージ**が `TexInfo.size` で行う（§8.2） |
| color | RGBA 0..1（パーツカラー頂点色 or α） |
| varg | `[rate, isMix, isMul, isAddSub]`（PartsColor ブレンド制御） |

- varg は PartColor の**自前 blend_type**（draw batch の blend ではない）から導出：Normal は per-corner `rate`、Mesh は corner-0 単一、Shape は Mix 固定（`rate = hasPc?1:0`）、Effect / PartColor 無しは Mul + rate 1（modulate）。PartColor blend は Mix/Mul/Add/Sub のみ有効、他は Mix クランプ（`parts_color_mode`）。**`(isMix,isMul,isAddSub)` は4通りしかないので `Prim.mode`（2bit）に畳み、頂点ステージの `varg_of` が展開する**——頂点あたり3 float を運ぶ理由が無い。
- **`position.z` は x,y に影響しない**。投影行列の第2列は `[0,0,1,0]`（`update_projection`）で、頂点ステージは `clip_position = vec4(pos.x, pos.y, 0.5, 1.0)` と z を捨てる。ただし**変換の途中では z を持つ**——インスタンス子の合成行列（`mat4_mul`）は3D回転を含みうるので、`t[8]*wz` が x に効く。捨てるのは最後だけ。
- `Prim` の色は **packed unorm8x4**（元データが u8 なので無損失）だが、**アルファだけは f32 のまま別配列**にしてある。PartColor 無しのパーツは α を runtime から f32 で受け取るので、量子化すると出力が動く。
- Normal パーツは4隅2三角形（`quad_corner` の `[0,1,2,2,1,3]`）。頂点変形/カラーで中心が効く場合に5頂点の扇へ切替する余地は残っている（WebGL 版と同じ判断）。

### 5.2 パイプライン設計（WebGL の動的 state → wgpu は焼き込み）

**wgpu では BlendState / DepthStencilState は RenderPipeline に焼き込む**。よって WebGL の `applyBlend`（動的 `blendFuncSeparate`）は消え、代わりに **組合せ分のパイプラインを事前生成して差し替える**。

```
パイプライン ≈ BlendType(12) × 用途{parts, effect, mask-write, mask-test, ss-shader×13}
> **✅ 実装済**: parts/effect（P2, 12ブレンド×stencil3）＋ mask-write/test（P3）＋ ss-shader 13種（P6, `(id_hash,blend)` 毎に遅延生成）。
```

**★P7 で確定：全パイプラインを遅延生成にする。** 現行は `SSInternalPlayer::new` が 12ブレンド × stencil3 = 36本＋マスク書き込み3本を**起動時に全部**コンパイルしており、1ブレンドしか使わないアニメでも全額払っている（初回表示レイテンシは §14 の WebGPU 弱点そのもの）。統合シェーダ側（`ss_pipelines`）が既に `(id_hash, blend)` キーの遅延生成を採用しているので、**同じ手法を content / mask 側にも広げる**だけ。

- キーは **`(kind, color_format, stencil_format)`**、`kind` は `Content(blend, stencil_test, geom_class)` / `Mask(op, geom_class)` / `Shader(id_hash, blend)`。フォーマットをキーに含めることで `SSRenderer` 自体がフォーマット非依存になり、**1 device に1個**で足りる（§1.2）。
- **`geom_class` は P7-6 で増えた次元**（Quad / Mesh / Shape / Effect）。頂点バッファを廃して展開を頂点ステージに移したので、幾何の種類＝頂点エントリポイントの違いになった（§5.5）。**遅延生成なので実測は増えていない**——Basic.ssab は1本、Shape.ssab で3本（`every_geometry_class_draws`）。
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

### 5.3 カラーブレンド WGSL（WebGL の `PartsColorProgram` を直訳）— ✅ 実装済（`renderer.rs` `fs_main`）

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

### 5.5 頂点生成経路（★P7-6・WebGPU 固有の本命）— ✅ 実装済（`scene.rs` ＋ `renderer.rs` `COMMON_WGSL`）

**旧実装は「WebGL でできることを wgpu で書いた」形**——毎フレーム CPU で `Vec<Vertex>` を組み立て（パーツ×4頂点×13 float の**散らばったストア**）、`write_buffer` で上げ、index buffer も毎フレーム作り直していた。ここが CPU 律速で、WebGL 版と同じ天井に当たっていた。

**WebGPU には storage buffer があるので、頂点を CPU で組む必要がない。**

ssruntime は `world_matrices` / `local_vertices` / `local_uvs` / `mesh_*` / shape 系を**フラットな f32 配列**で出している（§3.2）。これを**そのまま GPU へ渡し、頂点ステージで展開する**。現在の形:

```
[CPU] ノード毎: begin_node() が SoA スライスを scene アリーナへ extend_from_slice（memcpy）
                ＋ ノードの配置行列 16 float。返り値 NodeSlot が各配列の先頭オフセット
      パーツ毎: Prim（80 B）1本 — {arena オフセット4本, count, mode, alpha, z,
                colors[4] packed, alphas[4] f32, rates[4] f32}
      draw 毎 : {pipeline, texture, vertex_index 範囲}
[GPU] vs_quad / vs_mesh / vs_shape / vs_effect が vertex_index からパーツを引き、
      world_matrix × local_vertex → node transform → 投影 を実行
```

**`vertex_index` の割り当て**（頂点バッファも index buffer も無いので、これが唯一の入力）:

| class | 1 プリミティブあたり | vertex_index → プリミティブ |
|---|---|---|
| Quad | 6（`[0,1,2,2,1,3]`） | `vi / 6`。**完全に算術** |
| Effect | 6（パーティクル1枚＝1 prim） | `vi / 6`。runtime の index 列は必ずこの順（`lib.rs` の `[v,v+1,v+2,v+2,v+1,v+3]`）なので index 配列を運ばない |
| Shape | 30 固定（`SHAPE_FAN_VERTS`） | `vi / 30`。**余りの三角形は頂点0へ潰して面積ゼロ**にする。runtime の shape 配列は1パーツ 24 f32 = 最大12頂点＝最大10三角形なので 30 で必ず足りる（`push_shape_prim` が超過を弾く） |
| Mesh | 可変（runtime の三角形リスト） | 唯一算術で引けないので `(prim, vertex)` の**ペア列**（`mesh_stream`）を頂点ごとに1組持つ |

- **プリミティブ表は class をまたいで1本**。class ごとの `vertex_index` 空間に「他 class の prim が空けた穴」ができるが、そこは誰も draw しないので害が無い。おかげで prim のインデックスがフレーム内で一意になる。
- group(0) が「シーン」になった: 投影 uniform ＋ `f_arena` ＋ `prims` ＋ `mesh_stream`（storage 3本、`ShaderStages::VERTEX`）。バッファが再確保されたフレームだけ bind group を作り直す。
- **§1.1 の所有契約そのもの**：SoA は ssruntime 所有・Player は borrow。memcpy はまさに「borrow して書き出した時点で手放す」。
- インスタンスサブツリーの transform 合成は**ノード単位**（パーツ単位ではない）なので、`mat4_mul` の回数は変わっていない。合成結果はアリーナに1本置き、`Prim.node_base` で参照する。
- **WebGL2 は SSBO 無し・UBO 64KB・大規模動的 index 不可**なので、この形には到達できない。ssplayer-webgl と差がつく最大の一点であり、WebGPU を選ぶ意味そのもの（§14.1）。
- Player 単位の alpha は `Prim.alpha` 1つになった（旧実装は全頂点の色に畳み込んでいた）。**tint も同じ場所に足せる**。
- **★検証の要**: 頂点ステージの座標計算は**行列積ではなくスカラーの並び**で書いてある。旧 CPU 実装と**同じ順序で同じ演算**をさせるためで、これにより 134 枚のフィクスチャが**ビット完全一致**した（§13 `golden_corpus`）。ここを「綺麗な `mat4 * vec4`」に書き換えると FMA 収縮で 1 ULP ずれ、比較の基準を失う。書き換えるなら基準の取り直しから。
- 段階の先は **§14.2**（WebGL 由来の設計の棚卸し）へ引き継いだ。render bundle と timestamp query はそこの 5 と 10。

---

## §6 マスク（ステンシル）✅ 実装済（`libs/ssplayer-wgpu/src/mask.rs` ＋ `scene.rs`/`player.rs`）

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
- **外部参照は pack 名の文字列**（`PartTypeInstance::ref_anime_pack()`/`ref_anime_name()`）で解決＝ハッシュアルゴリズム不整合を回避。ホストは全 pack を `add_pack` / `add_ssab` で事前登録し、`set_pack` / `load_ssab` で主 pack を指定。ssab 型は `ssab::ssab::ss::format::{PartType, root_as_ss_anime_binary_unchecked}`。
- **どのパックが要るかは `.ssab` が知っている** ✅: `SSPack::instance_packs()` が `external_instances()` から**参照先パック名**を返す（自己参照と重複は除く）。テクスチャの `textures()` と同じ分担で、**パックが「何が要るか」を答え、ホストが取ってくる**。エクスポータは参照先パック自身の名前を書くので、その文字列が**ファイル名の stem** でもあり `ref_anime_pack` の照合キーでもある——だから `<.ssab のディレクトリ>/<name>.ssab` で必ず見つかる。`SS6PlayerForGodot` の `_load_external_ssabs`（`parent_dir.path_join(pack + ".ssab")`）と `ssplayer-core` の `resolveInstancePacks`（`<base>/<name>.ssab`）が同じ規則。
- **3面とも自動で解決する** ✅（§8.1）: `SSPlayer::load` / `load_with`（js の `load`、C の `ss_player_load`）が上記を辿って `add_pack` ＋そのパックのテクスチャロードまで済ませる。**Godot は子プレイヤーごとに `setSSABResource` を呼んで深い階層へ到達する**が、この Player は 1 プレイヤーがパック表をフラットに1枚持つ設計（`packs: HashMap<String, SSPack>`）なので、代わりに**推移的に辿る**。1階層＝1往復、その階層は並列取得。登録済み（`SSPlayer::has_pack`）は取りに行かないので、相互参照でも止まる。
- **transition スナップはホスト責務**: `InstanceSlot::step` は transition 時にセクション/ループ/方向を設定するのみで開始フレームへのスナップはしない。**`result.transitioned` 時に子 `context.play()` を呼ぶ**（呼ばないと独立モードが再生状態にならず進まない）。
- **スロットの step は「パーツ一覧」から回す。`draw_order` から回してはいけない（★実装済・回帰テストあり）**: ランタイムは `Hide` のパーツを `draw_order` / `draw_batches` から丸ごと落とす（`framedata.rs` の `drawn_as_self`）。描画走査のついでに step すると**非表示になった瞬間そのスロットが止まり**、独立再生の子／エフェクトは止まっていた時間だけ遅れて再開する（`10_instance.md` §Supporting Independent Playback、`20_effect.md`）。そこで `step_slots`（全スロットを再帰的に進める）と `render_node`（`draw_batches` を描く）を**別の走査**に分ける。可視性は「描くか」の判断にだけ使う。兄弟実装も同型（Web の `stepEffectSlots`/`stepInstanceSlots`、Godot の `_update_instance_children`）。
- **`parent_looped` は `Context::is_looped()`**: コントローラが `update` 中に立てるパルスで、次の `update` 開始時にクリアされるため**その `update` の直後に読む**。「フレームが後退したら」で代用しない——逆再生・ピンポンでは毎フレーム後退するため。子へは `result.child_looped` を伝播（独立再生の子は親ではなく自分のループが基準）。
- **子のサブツリーへ渡す delta は `delta * |ev.speed|`**: 独立再生の子は自分の時計をキーの speed で進めるので、その孫（子の中のエフェクト／インスタンス）も同じスケールで進めないと速度がずれる。
- 検証: `libs/ssplayer-wgpu/tests/slots.rs::a_hidden_independent_child_keeps_its_clock`（`Instance.ssab` の `INDEP_MID_INFINITE_HIDE`——独立・無限ループ・途中で Hide——の子フレームが Hide 中も進み続けることを固定する。draw_order 駆動に戻すと 23 tick 止まって落ちる）。ほか `libs/ssplayer-wgpu/tests/instance.rs`（ヘッドレス probe。同期/独立/pingpong の frame 推移を確認）。

### 7.2 エフェクト（別 Context を持たない）✅ 実装済

- エフェクトは**別 Context 不要**（親 Context の frame_no/delta で駆動するシミュレータのため上書き問題が無い）。`EffectSlot<'static>` を再生ノードが part_index で保持。`EffectSlot::new(resource, PartTypeEffect::ref_effect_name_hash(), seed=part_index)`。
- `slot.step(ev, frame_no, delta_secs, fps, parent_looped, y_flip=false, vert_stride=2)` → `ss_effect_step_result{ visible, draw_plan_buf: *const u8, draw_plan_len }`。`draw_plan_buf` はスロット所有・**そのスロットが次に step するまで**有効。step を描画走査から切り離した（§7.1）ため、プランは step 時に `EffectPart.plan`（再利用する `Vec<u8>`）へコピーし、描画走査はそれを読む。生ポインタで持ち回ると `SSPlayer: Send` が失われる——プレイヤー走査自体は直列に戻した（§14.2 項目6）ので Player 側の要求ではないが、**ホストが体をスレッド間で動かせる**性質は保つ。`ev = ctx.get_active_effect_event(part_index)`、`fps = ctx.get_fps()`。
- `EffectDrawPlan`（`root_as_effect_draw_plan_unchecked`）: `commands`（`cellmap_hash`/`quad_count`/`particle_offset`/`blend` 0=Mix 1=Add）、`vert_stride`、`verts`/`uvs`/`colors`/`indices`。スライス式（`po=particle_offset`）: verts `[po*4*stride..(po+qc)*4*stride]` / uvs `[po*4*2..]` / colors `[po*4*4..]` / indices `[po*6..]`（**quad-local 0基点、rebase 不要**）。
- **★エフェクト UV は正規化済み(0..1)**: スプライト/メッシュの pixel UV と違い、**tex_size で割ってはいけない**（probe で確認・当初バグを修正）。colors も正規化。
- verts は **slot-relative** → エフェクトパーツの world 行列を transform 合成（`mat4_mul(transform, effectPartWorld)`）。
- **1 command = 1 draw call**（`cmd.blend` でパイプライン切替、`cmd.cellmap_hash` でテクスチャ）。エフェクトは PartsColor を持たず、plan の色が既存 tex*color シェーダを変調するので varg 不要。実装: `libs/ssplayer-wgpu/src/scene.rs::append_effect`。

---

## §8 テクスチャ管理

- **hash → `{texture, width, height}`** の Map。textureHash は atlas 識別子（fnv1a）。**★P7: この Map は `SSRenderer`（device スコープ）が持つ**——同じキャラを N 体出しても VRAM もアップロードも1回で済ませるため（§1.2）。WebGL 版が `SSWebGLTextureCache` を surface 側に置いたのと同じ判断で、wgpu は pipeline 同様テクスチャも device 単位なので**共有範囲はさらに広い**（複数 canvas 間でも共有できる）。
- **premultiply を必ず適用**（SS 必須）。ネイティブは RGBA デコード後に自前で premul、または premul 済みテクスチャを供給。
  - **★注意（WebGPU の弱点）**: WebGL の `UNPACK_PREMULTIPLY_ALPHA_WEBGL` はドライバが無料でやってくれるが、**WebGPU の `writeTexture` に等価は無い**。CPU で premul するしかない。
  - **★P7 の対処（web）**: `Queue::copy_external_image_to_texture`（wgpu 29 にあり）＋ `createImageBitmap` を使えば、**デコードはブラウザのネイティブ実装（オフスレッド）、premultiply は GPU 側**に寄せられる。主目的はこの2点で、wasm から PNG デコーダ（`image` crate）が落ちるサイズ減はおまけ（§14.1 のとおりサイズは元々問題ではない）。
  - **ネイティブ側は CPU premul のまま。ただし一時コピーは0本**（✅ 実装済）。デコード結果の `Vec<u8>` を `upload_texture_owned` が**受け取って**その場で premul し、`Retention::Retain` ならその同じバッファを保持ピクセルにする（`renderer.rs` `upload_texture_owned` / `register_premultiplied` / `write_and_register`）。旧実装は retain 用と premul 用に `to_vec()` を2本取っており、2048² なら 16 MB × 2、4096² なら 64 MB × 2 の純粋な無駄だった。借用形の `upload_texture(&[u8])` は残置（C-API / web / engine 供給用）で、そちらは1本だけコピーする。
  - **premul はデコードワーカー側で済ませる**（✅ 実装済、§8.1）。ロード経路は premul 済みで返ってくるので、ホストのスレッドに残るのは `create_texture` ＋ `write_texture` だけになる。
- フィルタ/wrap: `ClampToEdge`（NPOT 対応）＋ `Linear`、mipmap なし。
- **texture_hash 0 / 未ロード → 1x1 白テクスチャ**にフォールバック（Shape content・純カラー対応）。**この白テクスチャは必ず白(255,255,255,255)で初期化すること**（`create_texture` だけだと透明黒で `tex*color=0` になり Shape が不可視）。`new()` は queue を持たないため `end_frame` 初回に `write_texture` で遅延初期化（§6）。

### 8.1 ssab の2形態に両対応（embedded / reference）— ✅ 実装済

ssconverter は `.ssab` を2形態で出力しうるので、SSPlayer はどちらでもロードできる必要がある。

| 形態 | 内容 | Player の扱い |
|---|---|---|
| **embedded** ✅ | テクスチャ（atlas PNG）を `.ssab` に埋め込み | **パース**（`embedded_images()` = エンコード済みバイト列を渡すだけ）と**デコード**（`decode_embedded()` = `image`、feature `embedded`）が分かれている。ネイティブは後者を使い `add_ssab` → 次の `update` で **premul → upload_texture** まで自動。**web は前者だけを使い、デコードはブラウザ**（自己完結・単一ファイル配布に最適・広告向け、はどちらでも変わらない） |
| **reference** ✅ | テクスチャは外部ファイルを名前/hash で参照 | Player は hash とパスだけ把握。**実バイトはホストが供給**（`upload_texture(hash, rgba, w, h)`）。ロード経路（ファイル/URL/バンドル）はホストの責務 |

- **ロード対象は `cellmaps()` と `external_textures()` の2つ（★必須）** ✅: `DrawBatch.texture_hash` は通常 `CellMap.name_hash` だが、作画で**テクスチャ変更（`TCHG`）**が指定されたパーツでは**テクスチャファイル名のハッシュ**になる。両方を同じマップへ入れること——移植ガイド `30_core/10_initialization.ja.md` §3 の `[!IMPORTANT]` そのもので、`SS6PlayerForGodot` の `_loadTextures` も両テーブルを走査している。**落とすと静かに壊れる**: 見つからない hash は 1x1 白フォールバック（§8）に落ちるので、そのパーツは消えるのではなく**白いシルエットで描かれ**、カウンタもエラーも動かない。`SSPack::textures()` / `embedded_images()` が両方を返す。フィクスチャは `assets/overall/TextureChange.ssab`（cellmap 1 ＋ external 3、うち1つは cellmap 自身の画像を別 hash で指す）。検証は `tests/pack.rs::a_pack_names_its_texture_change_targets`。
- **ハッシュ対応（重要）**: 埋め込みバイト列は `embedded_assets`（key = **ファイル名**のハッシュ）にある。cellmap は `image_path_hash` で引いて `name_hash` で返す**join が要る**が、`ExternalTexture` は `name_hash` がファイル名のハッシュなので**そのまま両用**（引く鍵と返す鍵が同じ）。この join は `embedded_images()` にあり、`decode_embedded_where` はその上に乗っている（1箇所だけ）。同じ画像が2つの hash で2回出てくることがあり、それが正しい——ホストが取得するときは**パス単位で重複排除**すること。
- **★ロードは1コール、3面とも同じ形（✅ 実装済）**。手でテクスチャを集めさせる API は採用しない——**利用者が使いたがらない**し、`external_textures()` のような取りこぼしが必ず起きる（静かに白シルエットになる）。

  ```rust
  player.load("assets/overall/Instance.ssab")?;   // Rust: 同期・GPU 引数ゼロ
  ```
  ```js
  await player.load('assets/Instance.ssab');     // js: 非同期はここ1箇所だけ
  ```
  ```c
  ss_player_load(player, "assets/Instance.ssab"); /* C */
  ```

  これで **パック ＋ 両テクスチャテーブル ＋ 参照先パック（推移的）** が揃う。置き場所が初期化であってアニメーション選択でないことは移植ガイド `30_core/10_initialization.ja.md` §3 が決めていて、`setAnimation` は同期のまま（切り替えのたびに `Promise` を挟まない）。
- **★`load` は device も queue も借りない**。読んだアトラスは**エンコード済みのまま** `pending_images` に積まれ、最初の `update` が `flush_textures` でデコード＋アップロードする——embedded がずっとそうしてきた経路に合流させただけ。借用モデル（device/queue が貸されるのは `update` から）を壊さずに1コールを実現する唯一の形で、デコード済み RGBA で持つと 2048² あたり 16 MB を初回フレームまで抱えることになる。**`load` 直後は `texture_size(hash)` がまだ `None`** なのはこのため。
- **統一供給 API**: 最終的にはどちらも「hash で1枚登録する」に集約される。embedded は Player 内部（`upload_texture`）またはホスト（`addTextureFromBytes`）、reference はホスト。ブラウザの `TexImageSource` 供給・ネイティブの RGBA 供給と対称。
- **借用モデルとの整合**: `add_ssab`/`load_ssab` は device/queue を借りないので、デコードは load 時（CPU）、GPU upload は次の `update`（queue 貸与時）に `pending_textures` を flush。
- **★フラッシュは1枚ずつではなくバッチで回す**（`decode.rs` `decode_all`、✅ 実装済）。`flush_textures` はまず**そのフラッシュで登録する全 hash を確定させてから**デコードに入る（reference の `pending_images` → パック名昇順の embedded、という §8.1 の優先順のまま）。理由は2つ:
  - **デコードを重ねられる**。4096² の PNG は1枚で 29〜51 ms あり、3枚のキャラクターは直列だとホストのイベントループを 86 ms 止める。`std::thread::scope` で重ねると**一番重い1枚の時間**まで縮む。premul（§5.2）もワーカー側でやるので、ホストのスレッドに残るのは `create_texture` ＋ `write_texture` だけ。常駐プールは作らない（§14.2 item 6 の但し書き）。
  - **順序が完了順に流されない**。「先に登録したものが勝つ」を「デコード後に `texture_size` を見る」で担保すると、並列化した瞬間に勝者がレースで決まってしまう。重複排除を**ディスパッチ時**に済ませ、結果は要求順に登録する。`decode_all` の戻り値が位置対応（デコードできなかった画像は `None` で、後続をずらさない）なのはこのため。
  - `web` は `embedded` を引かないので、このモジュールは web ビルドに一切入らない。
- **★デコーダはどの層に置くか（兄弟プレイヤーとの整合）**: **可搬コアには置かない**が3実装で一致している。SS6PlayerForCPP は `Common/` がピクセルを知らず、`Platform/OpenGL/ss6player_texture.cpp` の `Texture::CacheStore` が stb_image でデコードする（アプリ側で持つ経路も `_COMPILEOPTION_TEXTURE_EXTERNALENTITY_` で選べる）。ssplayer-core は `extractEmbeddedTextures` が**バイト列を返すだけ**で、`ssplayer-webgl` の `loadTextureFromBytes` が `createImageBitmap` に渡す——だから WebGL 版は**デコーダを同梱せずに両形態に対応できている**。本実装の `embedded_images()` / `decode_embedded()` の分割はこれに合わせたもの。
- **feature `embedded`（default on。ただし `web` は引かない）**: Player 内蔵 `image`（PNG のみ）でデコード。**`web` feature はこれを引かない**——ブラウザには既にデコーダがあり、`image` を積むのは二重持ちだから。**この変更の効果（2026-07 実測、wasm-pack release + `wasm-opt -Oz`）: brotli 201,386 B → 142,418 B（−29.3%）、gzip 249,331 → 178,011（−28.6%）**。ブラウザ経路の実装ぶんを戻した当時の最終形が **brotli 147,161 B**。**以上は `image` 除去の前後を測った当時の値**であって現在値ではない——その後に入った機能ぶんで wasm は伸びており、**現在は wasm 単体で brotli 165,896 B（raw 580,700 B）**。内訳と経緯は §14.1。自分でグルーコードを書きたくないページは `web` に `embedded` を足せば従来どおり動く。
- **検証**: `render.rs::render_embedded_ssab`（埋め込み `Basic.ssab` をテクスチャ無供給でロード → reference レンダーとピクセル完全一致 14378/14378）。埋め込みフィクスチャは `assets/overall_emb/`（`ssconverter-cli --embedded` 生成）。

### 8.2 SSPack とテクスチャ供給（★P7-5）— **GPU テクスチャは所有しない** ✅ 実装済（`libs/ssplayer-wgpu/src/pack.rs`）

「同じキャラを何体も出す」が普通になるので `.ssab` の共有単位を作る。ただし **`SSPack` は CPU データだけを持ち、GPU テクスチャは所有しない。**

```rust
pub struct SSPack { /* Arc<str> 名前, Arc<SsabResource>, Arc<[TextureRef]> 必要アトラス, embedded 有無 */ }
pub struct TextureRef { hash: u32, path: String, embedded: bool }  // ★何が要るか＋どこにあるか

SSPack::load(bytes) -> Result<SSPack, SSRuntimeError>       // ★device も queue も要らない（CPU のみ）
SSPack::load_named(name, bytes) -> Result<SSPack, _>        // ホストが名前を決める場合
pack.name() -> &str;                                        // ★既定は .ssab 自身の name
pack.textures() -> &[TextureRef];                           // ★cellmaps() ＋ external_textures()（TCHG 先。§8.1）
pack.has_embedded_textures() -> bool;
pack.instance_packs() -> Vec<String>;                       // ★インスタンスパーツの参照先パック名（§7.1）
pack.animation_names() -> Vec<String>;
pack.embedded_images() -> Vec<EmbeddedImage<'_>>;           // ★エンコード済みバイト列を**借りるだけ**（デコーダ不要・feature 不問）
pack.decode_embedded() -> Vec<EmbeddedTexture>;             // embedded の場合のみ。ピクセルを**渡すだけ**（feature `embedded`）
pack.decode_embedded_where(|hash| ...) -> Vec<EmbeddedTexture>; // ★デコード前に絞る

player.set_pack(&pack);   // Arc をクローンして参照するだけ（主パック指定）
player.add_pack(&pack);   // 外部インスタンス参照解決用
player.textures(None) -> Option<&[TextureRef]>;             // 主パック（Some(name) で登録済みパック指定）
```

- **reference 形態のアトラスの在り処は `.ssab` が知っている（★兄弟プレイヤーと同型）** ✅: `TextureRef.path` は exporter が書いた画像パスで、**`.ssab` 自身の位置からの相対**。ホストはこれを読み込み元ディレクトリと結合するだけでアトラスに辿り着けるので、「ディレクトリ内の `*.png` を総当たりで舐めてファイル名 stem から hash を作る」という推測（旧 viewer / 旧テストのやり方。converter がたまたま cellmap 名で出力するから動いていただけ）が要らない。ssplayer-core の `extractTexturePaths` + `loadTextures` と同じ分担で、**パックが「何をどこから」を答え、ホストが読む**。`embedded` が true のエントリはホストの仕事が無い（次の `update` でパックが渡す）。**任意の場所からドロップされた `.ssab` が再生できる**のはこの性質のおかげ（`viewer-metal-cpp` の D&D、§13）。
- **パック名は `.ssab` が持っている**（`SsAnimeBinary.name`）。インスタンス参照は `ref_anime_pack` 文字列で解決する（§7.1）ので、ホストがファイル名から名前を復元する必要はない——`SSPack::load` の既定名がそのまま噛み合う。`load_named` はファイル名で管理したいホスト向け（`add_ssab` / `load_ssab` はこちらの薄いラッパ）。
- **デコードは「渡すだけ」でキャッシュしない**。代わりに `decode_embedded_where` の述語が **PNG デコードの前**に走るので、既に登録済みのアトラスは**デコードごとスキップ**される。`SSPlayer` はこれを使って `update` 時に「まだ無いものだけ」を上げる。

**なぜ GPU テクスチャを持たせないか（★設計判断）**: 持たせると `SSPack` の refcount が **engine のアセット管理と二重の所有系**になる。Bevy なら `Assets<Image>` / `Handle<Image>` / `RenderAssets<GpuImage>` が既にライフサイクル・ホットリロード・アンロードを握っており、そこへ第二の所有者を持ち込むと (1) 同じアトラスが Bevy 側と我々側で**二重アップロード**、(2) 解放タイミングが engine の意図とずれる、(3) フォーマット・mip・サンプラの方針を engine から奪う、という3点で必ず衝突する。

**★`AssetLoader` — 面ごとに差し替わるのはこの1枚だけ（✅ 実装済、`player.rs`）**:

```rust
pub trait AssetLoader {
    fn reset_base(&mut self);                   // ★構築時の root に戻る（前のロードの base を捨てる）
    fn set_base(&mut self, src: &str);          // ★src の在り処に根付き直す（ディレクトリの切り出しは実装の仕事）
    async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>>;  // ★まとめて渡す（並列化の余地は実装側）
    async fn supply_texture(&mut self, hash: u32, encoded: &[u8]);
    fn has_texture(&self, hash: u32) -> bool;   // 取りに行かなくて済むかのヒント
    fn warn(&self, message: &str);              // 1枚欠けても続行（ERR_PRINT / console.warn 相当）
}
```

**★パス解決はローダの仕事で、走査は文字列を1つも組み立てない**（契約は `tests/loader_contract.rs`）:

0. `load_with` はまず `reset_base()` を呼ぶ。ローダは**構築時の root** に戻り、前のロードが動かした base は捨てられる。
1. 最初の `read` は `src` を**そのまま**。このときローダは呼び出し側が置いた場所に根付いている。
2. 続いて `set_base(src)` が**1回だけ**呼ばれる。以降 `read` に渡るのは**裸の名前**——エクスポータが書いたアトラスパスと `<pack 名>.ssab`——であって、走査側が何かを前置することは無い。
3. `load_assets` は `reset_base` も `set_base` も呼ばない。バイト列がどこから来たか知らない入口なので、**ローダは呼ぶ側が根付かせておく**（web の `loadSsab(bytes, pack?, baseUrl?)` がまさにこれ）。

**1 が 2 より先であることが安全性の本体**。`src` を読む時点で `src` 由来の base はまだ存在しないので、「`src` を自分自身に対して解決する」という過去のバグが**書きようがない**（テストで見つける対象ではなくなった）。

**0 は 1 の前提**。`set_base` は「構築時の root から導く」——前回の答えに畳み込まない——ので冪等であり、`reset_base` と対で**1つのローダが何回でもロードに使える**。これが無いと base が積み上がり、2回目の `load_with(loader, "overall/Other.ssab")` が `overall/overall/` を見に行って落ちる。engine アダプタが `AssetServer` / `ResourceLoader` / `.pck` ハンドルを1つのローダに包んで全キャラをロードする形がまさにこれで、`::one_loader_serves_a_second_load` が固定している。

**なぜ走査側で切らないか（★設計判断）**: パスの語彙を知っているのはローダだけで、走査は3面（fs / URL / アーカイブ）を同時に相手にする以上どれか1つの語彙しか選べない。セパレータの判断を走査側に置くと、その1つ以外が黙って壊れる——base が1階層ずれてもエラーもカウンタも動かず、アトラスが白シルエットになるだけなので気づけない。`Path::parent` は Windows で `\` を切り POSIX で切らない、`URL` は最終セグメントを置き換える、と**それぞれが自分の語彙で正しく答える**ので、走査は何も組み立てない。**走査が `src` の中身を読まない**ことは `::what_is_asked_for_does_not_depend_on_how_src_is_spelled` が固定している。

出荷実装は2つ——`SSPlayer::load` の裏の fs（`assets.rs`）と web 面の `fetch` ＋ `createImageBitmap`（後者はピクセルが wasm メモリを一度も通らない、§10）。**走査（両テクスチャテーブル／パス単位の重複排除／参照パックの推移的解決）は `player.rs` に1本だけ**あり、面が半分だけ実装することが構造的にできない。SS6PlayerForCPP の `Common/` と `Platform/`、`ssplayer-core` の `IResourceManager` と同じ線。

**なぜ fs 直叩きにしないか（★設計判断）**: 「`std` に依存しているのだから Player が fs を読めばいい」は**ネイティブ単体でしか成り立たない**。`std::fs` は wasm32 でコンパイルは通り実行時に必ず失敗する。さらに engine 統合先の過半でファイルは fs に無い——**Godot はエクスポートすると `.pck` の中**（だから `SSPlayerForGodot` は `ResourceLoader::load`）、**Unity の Android は APK の中**、Unreal は `.pak` ＋ `IFileManager`、Bevy は `AssetSource`（バイナリ埋め込み・加工済みアセット・wasm ビルド）。読み口を1つ差し替えられれば全部通る。

**テクスチャは所有ではなく借用チャネルにする**（Device/Queue と同じ扱い＝借用モデルの一貫適用）:

```rust
// 供給側が所有する TextureView を借りる。renderer はこれで bind group だけ作る
renderer.set_texture(&device, hash: u32, view: &wgpu::TextureView, size: (u32,u32), alpha: AlphaMode);
renderer.forget_texture(hash);                  // 借用を返す（供給側の解放を遅らせない）
renderer.texture_alpha_mode(hash) -> Option<AlphaMode>;

// 便利経路（viewer / web / ffi）: renderer が自分でテクスチャを作る（CPU premul 付き）
renderer.upload_texture(&device, &queue, hash, rgba, (w, h), retention);
```

- **Bevy 統合**: アダプタが `GpuImage.texture_view` を毎回 `set_texture` で渡すだけ。二重アップロードなし、解放は Bevy の意志どおり。embedded ssab の場合も `decode_embedded()` の**ピクセルを `Assets<Image>` へ流し込めば**、テクスチャは最初から Bevy の資産になる。
- **wgpu の型は内部が refcount** なので `TextureView` を clone して保持しても dangling しない。ただし**我々が握っている間は解放が遅れる**——engine 側で不要になったら `renderer.forget_texture(hash)` を呼ぶ、という契約にする。
- **`AlphaMode`（premul / straight）を per-texture で持つ** ✅。engine が供給するテクスチャは premultiply されていないのが普通で、現行の「アップロード時に CPU で premul」（§8 冒頭）が使えないため。シェーダの逆premul（`tex.rgb / tex.a`）を**このフラグで飛ばす**だけで済み、パイプラインは増やさない（`renderer.rs` `ss_straight_rgb` / `ssTex` の `select`）。→ フラグはテクスチャ bind group 側の小さな uniform（group(1) binding(2) の `TexInfo`、32 B）に置く。**同じ uniform にテクスチャサイズ `(w, h, 1/w, 1/h)` も入れてあり**、現在 CPU でやっている UV 正規化を GPU へ移す器はもう出来ている（§5.5 と相性が良い。可視性は `VERTEX_FRAGMENT`）。
- **重複排除は hash マップ側の性質**なので、`SSPack` が GPU を持たなくても「同じキャラ N 体でアップロード1回」は成立する（`SSRenderer` の `hash → entry` が1つだから）。**登録済みが勝つ**——2体目はデコードもアップロードもせず、ホストが自前で供給した hash も奪われない。逆に embedded 側で上書きしたいホストは先に `forget_texture` を呼ぶ。
- **device 喪失復帰の責務分界**（§14）: `upload_texture` 経路（我々が作った）は `SSRenderer` が retained source から復帰させる。`set_texture` 経路（借り物）は**供給側の責務**——engine は自分の asset を作り直したうえで `set_texture` を再度呼ぶ。境界を曖昧にしない。1つの hash を両経路が同時に主張しないよう、`set_texture` は既存の retained source を捨てる。

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

### 9.1 SSRenderer（層2a・device スコープ）✅ 実装済（`libs/ssplayer-wgpu/src/renderer.rs`）

**device に1個**。フォーマット非依存で、pipeline は使われた組合せだけ遅延生成する（§5.2）。

```rust
pub struct SSRenderer { /* lazy pipelines, layouts, sampler, white fallback, texture cache */ }

impl SSRenderer {
    pub fn new(device: &wgpu::Device) -> Self;                      // ★config を取らない
    pub fn pipeline_count(&self) -> usize;                          // 診断（§5.2）

    // --- テクスチャ（借用チャネル＝主、所有は便利経路。§8.2）---
    pub fn set_texture(&mut self, device, hash: u32, view: &wgpu::TextureView, size: (u32,u32), alpha: AlphaMode);
    pub fn forget_texture(&mut self, hash: u32);
    pub fn texture_alpha_mode(&self, hash: u32) -> Option<AlphaMode>;
    pub fn upload_texture(&mut self, device, queue, hash: u32, rgba: &[u8], size: (u32,u32), retention: Retention);
    pub fn texture_size(&self, hash: u32) -> Option<(f32, f32)>;
    pub fn texture_count(&self) -> usize;                           // 診断
    pub fn texture_upload_count(&self) -> usize;                    // 診断（§13：N体で1回を測る）
    pub fn forget_texture_sources(&mut self);                       // 保持コピーを捨てる（§14）

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

/// テクスチャの色とアルファの関係（§8.2）。パイプラインではなく
/// テクスチャ側の uniform に載るので、混在しても本数は増えない。
pub enum AlphaMode {
    /// rgb は a 済み。我々の `upload_texture` はこちら。
    Premultiplied,
    /// rgb は a と独立（PNG デコード直後 ＝ engine が渡してくる普通の形）。
    Straight,
}
```

**復帰の責務分界（★7-3 で確定）**: device 喪失時、ホストは **`SSRenderer::restore_device` を1回**呼び、続いて各 Player の `restore_device(&mut renderer, ..)` を呼ぶ。前者が pipeline / テクスチャ（`Retain` 分）を、後者が投影 uniform とフレームアリーナを、そして embedded アトラスの再デコード（`Reproducible` 分）を受け持つ。**旧設計はこの全部を Player ごとに繰り返していた**——アトラスの PNG デコードを含めて。

- パイプラインキャッシュのキーは `(blend, stencil_test, color_format, stencil_format)`。フォーマットをキーに入れることで、**フォーマットの違う複数ターゲットが1つのレンダラを共有できる**。
- 旧 `SSInternalPlayer` の「`new()` で作られ以後不変」な部分がここに来る。

### 9.2 SSScene（層2b・描画ターゲットスコープ）✅ 実装済（`libs/ssplayer-wgpu/src/scene.rs`）

**描画ターゲット1枚に1個**。投影・フレーム頂点アリーナ・draw_call 列・ステンシル管理を持ち、**order 昇順で N 体を1 pass にまとめる**。

```rust
pub struct SSScene { /* projection UB, frame arena / prims / mesh_stream, draw_calls, ss params */ }

impl SSScene {
    pub fn new(device: &wgpu::Device, config: &SceneConfig) -> Self;   // ★renderer を取らない（§5.5）
    pub fn set_viewport(&mut self, queue: &wgpu::Queue, w: f32, h: f32, y_down: bool);

    /// 全 Player を進め、1本のアリーナへ積んで GPU へ上げる。
    /// players は order 昇順に整列してから走査（同値は与えられた順を保つ）。
    pub fn update(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue,
                  delta_secs: f32, players: &mut [&mut SSPlayer]);

    /// ホストが開いた RenderPass へ積むだけ。begin/submit/present はホスト責務。
    pub fn draw<'a>(&'a self, renderer: &'a SSRenderer, pass: &mut wgpu::RenderPass<'a>) -> usize;

    pub fn restore_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue);

    // --- 層3（SSPlayer の木走査）が積むための下位 API（§5.5）---
    pub fn begin_node(&mut self, frame: &FrameView, transform: &[f32;16]) -> NodeSlot;  // SoA を memcpy
    pub fn push_transform(&mut self, transform: &[f32;16]) -> NodeSlot;                 // エフェクト用
    pub fn append_batch(&mut self, renderer: &SSRenderer, frame: &FrameView, node: &NodeSlot, batch: &BatchInfo);
    pub fn append_mask_writer(&mut self, frame: &FrameView, node: &NodeSlot, writer: &MaskWriter, op: MaskOp);
    pub fn append_effect(&mut self, plan_buf: &[u8], node: &NodeSlot, stencil: StencilTest, alpha: f32);
}
```

- **`new` / `restore_device` は `SSRenderer` を取らない**。group(0) の bind group はフレームのバッファが確定する `end_frame` で、**その時に描いているレンダラの layout に対して**作るため。構築時のレンダラを覚え込む必要が無くなった（P7-6）。

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

    // --- ロード（★1コールで全部揃う。§8.1）---
    pub fn load(&mut self, path: impl AsRef<Path>) -> Result<(), SSRuntimeError>;  // ★既定。fs。GPU 引数なし
    pub async fn load_with(&mut self, loader: &mut impl AssetLoader, src: &str) -> Result<(), SSRuntimeError>;  // 差し替え版
    pub async fn load_assets(&mut self, loader: &mut impl AssetLoader, pack_name: &str);  // 登録済みパックの資産だけ（★ローダは呼ぶ側が根付かせる）

    // --- pack 登録 / 再生制御（★P7-5 実装済）---
    pub fn add_pack(&mut self, pack: &SSPack);      // 外部参照解決用に全 pack 登録（§8.2）
    pub fn has_pack(&self, name: &str) -> bool;    // 自前で解決するホスト向け（§7.1）
    pub fn set_pack(&mut self, pack: &SSPack);      // 主 pack 指定
    pub fn add_ssab(&mut self, pack_name: &str, data: &[u8]) -> Result<(), SSRuntimeError>;   // = load_named + add_pack
    pub fn load_ssab(&mut self, pack_name: &str, data: &[u8]) -> Result<(), SSRuntimeError>;  // = load_named + set_pack
    pub fn set_animation(&mut self, anim: &str) -> Result<(), SSRuntimeError>;  // 選択＝インスタンス木を再構築（★確保はここだけ）
    pub fn play(&mut self);   // 開始 / pause からの再開。引数なし・確保なし
    pub fn pause(&mut self);  pub fn stop(&mut self);  pub fn set_speed(&mut self, speed: f32);
    pub fn is_playing(&self) -> bool;
    pub fn animation_names(&self) -> Option<Vec<String>>;
    pub fn canvas_size(&self) -> Option<(f32, f32)>;

    // --- トランスポート（★実装済）---
    pub fn is_paused(&self) -> bool;  pub fn is_finished(&self) -> bool;  pub fn is_looped(&self) -> bool;
    pub fn seek(&mut self, frame: f32);   // 小数フレーム可
    pub fn frame_no(&self) -> f32;
    pub fn start_frame(&self) -> i32;  pub fn end_frame(&self) -> i32;  pub fn total_frames(&self) -> i32;  // 区間を反映・両端含み
    pub fn set_loop_count(&mut self, count: i32);  pub fn loop_count(&self) -> i32;  pub fn remaining_loops(&self) -> i32;
    pub fn set_frame_rate(&mut self, fps: i32);  pub fn frame_rate(&self) -> i32;
    pub fn set_animation_section(&mut self, start: i32, end: i32);  // ★set_animation の後に呼ぶ
    pub fn set_playback_direction(&mut self, direction: PlaybackDirection, style: PlaybackStyle);
    pub fn playback_direction(&self) -> PlaybackDirection;  pub fn playback_style(&self) -> PlaybackStyle;
    pub fn set_skip_frames(&mut self, skip: bool);  pub fn skip_frames(&self) -> bool;

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

**パーツ単位オーバーライド（★実装済・§9.5）**:

```rust
pub fn find_part_index(&self, name: &str) -> Option<u32>;   // 名前→index。index はパック内で安定
pub fn set_part_visibility_override(&mut self, part: u32, hide: bool, cascade: bool) -> Result<(), SSRuntimeError>;
pub fn clear_part_visibility_override(&mut self, part: u32) -> Result<(), SSRuntimeError>;
pub fn set_part_color_override(&mut self, part: u32, blend_op: BlendOp, rgba: u32, priority: OverridePriority) -> Result<(), SSRuntimeError>;
pub fn set_part_color_override_corners(&mut self, part: u32, blend_op: BlendOp, corners: [[u8;4];4], priority: OverridePriority) -> Result<(), SSRuntimeError>;  // lt, rt, lb, rb
pub fn clear_part_color_override(&mut self, part: u32) -> Result<(), SSRuntimeError>;
pub fn set_part_cell_override(&mut self, part: u32, cellmap_name: &str, cell_name: &str, priority: OverridePriority) -> Result<(), SSRuntimeError>;  // 名前は内部で FNV-1a
pub fn clear_part_cell_override(&mut self, part: u32) -> Result<(), SSRuntimeError>;
pub fn clear_all_part_overrides(&mut self);
```

- **index はルートアニメを指す**。`BlendOp`（0=Mix 1=Mul 2=Add 3=Sub）/ `OverridePriority`（0=次キーフレームまで 1=次アニメまで 2=永続）も `PlaybackDirection` と同じ理由で本クレート定義＋`as_wire`/`from_wire`。範囲外は `None` → js/ffi ではエラーにする（黙って別のブレンドにしない）。
- **インスタンスパーツの中には届かない**。子は独自 Context（§7.1）なので、インスタンスを載せているパーツへのオーバーライドは子の描画を変えない。
- **オーバーライドは `set_animation` の後**。ランタイムがオーバーライド表を確保するのが `set_animation` で、そこが `Permanent` 以外を消す場所でもある。前に呼ぶと（黙って失われるのではなく）エラーになる。
- 表の寿命はランタイム所有。Player 側でキャッシュも毎フレーム再適用もしない。

**パーツ読み出し（★実装済）**:

```rust
pub fn part_count(&self) -> usize;                          // ルートアニメのパーツ数
pub fn part_transform(&self, part: u32) -> Option<[f32; 6]>; // [m00, m01, m10, m11, tx, ty]
```

- 並びと順序は ssplayer-webgl の `getPartTransform` と同一（`x' = m00·x + m01·y + tx`）。**アニメローカル・Y-down** で、`set_transform` もシーンの投影も掛かっていない。ホストが自分の座標系へ合成する。
- `SSScene::update` の**後**に読む（直近の update が作ったフレームを表す）。**hide のパーツも位置を返す**（描かれないだけで行列は計算されている）。
- index は範囲チェック済み。オーバーライド系はランタイムが弾くので不要だが、こちらはランタイムのバッファを直読みするため、範囲外は NaN ではなく `None` にする必要がある。
- コピーを返すので `FrameView` と違って保持してよい。

**`is_part_hidden` は未実装**: hide フラグは FrameData 側にあり、`Context::get_frame_data` が `&mut self` なので、クエリが `&mut` になるか、Godot のように描画ウォークで hide 表をキャッシュするかの選択になる。前者は API として不自然、後者は毎フレームのコストなので、必要が出た時点で決める。

**イベント（★実装済・§9.5）**:

```rust
pub fn looped_last_update(&self) -> bool;    // ループ境界を跨いだ（update 1回につき1報）
pub fn finished_last_update(&self) -> bool;  // 再生完了。★立ち上がりエッジ（1 update だけ true）
pub fn events(&self) -> &[SsEvent];          // UserData / Signal / Audio（所有型にデコード済み）
```

**通知はコールバックではなくポーリング**。`SSScene::update` の中で Player に溜め、ホストが直後に読む。借用モデルはホストがクロックを回す形なので元々これに合っており、かつコールバックだと js（関数オブジェクト）と ffi（関数ポインタ + user_data）で API の形が割れる。三面とも同じ形にできるのがポーリングだけ。

以下は SDK の Player 移植ガイド（`docs/30_core/20_playback_control.md`）が定める全 Player 共通の契約。**「複数の Player で実際に起きた欠陥」として列挙されている**ので、逸脱しないこと。

- **フレームスキップで飛ばしたフレームのイベントも発火する。** ランタイムは通過フレームを全部記録する（`passed_frames`。SDK 自身のテストが長い tick で `[4,5,6,7,8]`、ループ跨ぎで `[3,4,5,0,1]` を固定）。update 直後に `get_passed_event_indices()` を読むだけでよく、**自前でフレームを走査してはいけない**（着地フレームだけ見るのも同じく取りこぼす）。
- **`is_looped` はパルス**（update 先頭でクリア）。update 直後にそのまま読む。1 update で複数周した場合も 1 報にまとまる。
- **`is_finished` は状態**（次の `play`/`setup_animation` までスティッキー）。**必ず立ち上がりエッジで通知**し、`play()` でラッチ（`prev_finished`）を戻す。生で通知すると完了後は毎フレーム発火し、ラッチを戻し忘れると**次の**完了を取りこぼす。無限ループでは発火せず、`stop()` でも発火しない。`!is_playing` や「最終フレーム到達」で代用しないこと（`end_frame_reached` は誤用が続いたため C-API から削除済み）。
- **イベント収集は描画をスキップし得る処理より前に置く。** Godot は「整数フレームが変わったか」で門を作り、`play()` 直後のフレーム 0 のイベントを落とした。本実装は `context.update()` の直後、`step_slots`/`render_node` の前で集める。
- **非表示（`set_visible(false)`）の tick は「何も起きていない」と報告する（✅ 修正済）。** 非表示プレイヤーはタイムラインを進めない＝報告すべき tick が無いのに、報告フィールドを前回のまま残していたため、`events()` が**最後に見えていた tick のイベントを毎 update 返し続けた**——イベント毎に音を鳴らすホストは、カリングされている間ずっと鳴らす。`build_frame` は早期 return の**前**に `events` / `looped` / `finished_edge` をクリアする。`prev_finished` は**残す**（消費済みの完了エッジを再表示時に再発火させないため）。`set_animation` も同じ理由でクリアする（別のアニメの tick を引き継がない）。回帰は `tests/events.rs::a_hidden_player_reports_no_events`。
- **イベント index はアニメの `events()` サイズで範囲チェックする。** -1 や別アニメ由来の index が FlatBuffer 外を読む。

イベントは**所有型にデコードして返す**（`FrameView` と違い保持してよい）。ホストの反応（音を鳴らす、当たり判定を出す、スクリプトを進める）は raise した tick より長生きするのが普通なため。生 index を返すだけの形（Flutter の `onEvents` 相当）は採らない。

**残りの将来 API（webgl 版に追随）**: `box`（追従矩形）/ `clip`（scissor）/ `backdrop`（framebuffer 読み取り系ブレンド用の下地）。§9.5。

**トランスポートの enum（★確定）**: `PlaybackDirection` / `PlaybackStyle` は **ssruntime の同名 enum を re-export せず、本クレートで定義**して境界で変換する。ssruntime 側は掛け算に使うため `Forward = 1 / Backward = -1` だが、**C-API・wasm・他の全 Player は `0 / 1`**（`ffi/mod.rs` の `ss_runtime_set_playback_direction` が `direction == 0` で判定）。re-export すると `-1` が js / ffi 面に漏れて全兄弟と食い違うので、ワイヤ値は `as_wire` / `from_wire` に固定した。`tests/transport.rs` がこれを固定している。

**`set_animation` は木を作り直す**ので、上記トランスポート設定と `set_speed` はすべてパック既定へ戻る。呼び出し側が毎回設定し直す。Flutter / Unity Player のようにプレイヤー側でキャッシュして再適用する層は**現時点では持たない**（本クレートはホスト駆動の低レベル API という位置づけのため）。特に **`set_animation_section` は `set_animation` の後**に呼ぶこと。

> **現状と将来**: エラーは当面 `SSRuntimeError` を流用。将来 `SSPlayerError`（`thiserror`）でラップし FFI 整数コード / JS `JsError` へ変換。ssab の **embedded 自動テクスチャロード**（§8.1）は ✅ 実装済。**seek** ✅ 実装済。

### 9.4 各ホストからの駆動（rs / js / ffi 共通の型）

ホストが所有するのは **wgpu 一式 ＋ `SSRenderer` ＋ Player の置き場**。共通ループは `scene.update(&mut renderer, …, &mut players)` → `begin_render_pass` → `scene.draw(&renderer, &mut pass)`。

- **① rs / viewer**: winit で Surface を所有 → 毎フレーム `get_current_texture` → 上記ループ → `submit`/`present`。
- **① rs / Bevy**: Bevy の `RenderDevice`/`RenderQueue`（内部は wgpu）を借りる。`SSRenderer` は Bevy の `Resource`、`SSScene` は描画先ごと、`SSPlayer` は **Component**（ECS に住む）——`SSScene` が Player を所有せず借りる設計（§1.2）はここのため。**テクスチャは Bevy の `Handle<Image>`/`GpuImage` をそのまま借りる**（§8.2）。
- **② js / web**: `web.rs` が canvas から Surface を生成・所有。**device と `SSRenderer` はページで1つ**を共有し、canvas ごとに `SSScene` を持つ（§10）。
- **③ ffi / C・C++**: `host.rs` が raw-window-handle から Surface/Device を生成・所有し、`ss_scene_*` / `ss_player_*` ハンドル API 越しに同じループを回す（§11）。

### 9.5 将来 API（REDESIGN_ASSESSMENT §7）

パーツ単位オーバーライド（色/セル/表示）、ミキシング、ステートマシン。いずれも ssruntime 側の合成で完結し FrameData/DrawBatch 契約を壊さないため、`SSPlayer` に制御メソッドを足すだけで層2 の改修は不要。ssplayer-webgl は既に実装済（`setPartColorOverride` / `setPartCellOverride` / `setPartVisibilityOverride` / `getPartTransform`）なので、移植先の形はそちらに合わせる。

**進捗**: **パーツ単位オーバーライド ✅ / `getPartTransform`（= `part_transform`）✅ 実装済**（いずれも rs / js / ffi の三面。API と決定事項は §9.3）。上の見立てどおり層2 の改修は不要だった。**イベント ✅ 実装済**（ポーリング方式。契約は §9.3）。残りは `box` / `clip` / `backdrop`（描画側）、`is_part_hidden`（§9.3 の注記）、ミキシング / ステートマシン（前者は SDK に primitives が揃っており、後者は SDK 側 Phase 3 待ち）。

---

## §10 WebGPU / JS 面（② js — Rust API の上のホスト）— ✅ P7-9 実装済（`libs/ssplayer-wgpu/src/web.rs`）

- **単一 Rust→WASM バイナリ**（ssruntime + 層2 + player を wasm-bindgen で1つに）。wgpu が WebGPU バックエンドを叩く。
- `libs/ssplayer-wgpu/src/web.rs`（`#[cfg(feature="web")]`）が **wgpu 一式を生成・所有**し、借用モデルの `SSScene`/`SSPlayer`（§9）を駆動する薄いホスト。wasm-bindgen 束縛はこの層。
- **★P7-9: device と `SSRenderer` はページで1つ**。旧 `SsPlayerWeb` は `create(canvas)` ごとに adapter/device を要求しており、1ページに N 要素置くと **N 個の GPUDevice** ができていた（device をまたぐとテクスチャも pipeline も共有できず、`restore_device` も N 回走る）。**§1.2 のスコープ契約をそのまま3オブジェクトに落とす**:

| JS 型 | 個数 | 持つもの |
|---|---|---|
| `SsContextWeb` | **ページで1つ** | instance / adapter / device / queue ＋ `SSRenderer`（pipeline ＋ アトラス） |
| `SsSurfaceWeb` | canvas ごと | `wgpu::Surface` ＋ `SSScene`（投影・フレームアリーナ）＋ その canvas の player 群 |
| `SsPlayerWeb` | 何体でも | 再生木だけ。**GPU リソースを持たない**ので安い |

```js
import { sharedContext } from './ss-context.js';

const ctx     = await sharedContext();          // ページで1回（内部で SsContextWeb.create()）
const surface = ctx.createSurface(canvas);      // canvas ごと
const player  = surface.createPlayer();         // 1 canvas に何体でも

await player.load('assets/Basic.ssab');         // ★最短形: パック＋アトラス＋兄弟 ssab（§8.1/§7.1）
await player.loadSsab(bytes, null, ssabUrl);    // bytes から入る場合（D&D・バンドル）
player.setAnimation('idle');                    // 選択＝確保はここだけ
player.play();                                  // 開始 / pause からの再開（§9.3）
player.setTransform(x, y, scaleX, scaleY, rotation);
player.setOrder(1);                             // 小さいほど先に描く
surface.frame(dt);                              // 1コールで全 player を1パスに
player.destroy(); surface.destroy();            // ★明示。実体は ctx にあるので GC では消えない
```

- **実装は「1つの `Rc<RefCell<WebContext>>` ＋ 世代付きハンドル」**（`roster.rs`、C 面と同じ理由＝§11）。JS は Rust の `&mut` を持てないので名前で引く必要があり、破棄済みハンドルは**後継ではなく「破棄済み」に解決**しなければならない。セルを1つに絞ってあるのは、借用を跨いで別の借用を取る経路を作らないため。
- **`destroy()` が要る**のは所有が context 側にあるから。JS ラッパを捨てても swapchain は消えないので、要素の `disconnectedCallback` は player → surface の順に明示破棄する。
- **device 喪失復帰はページで1回**（`ctx.restoreDevice()`）。喪失は device のものなので全 canvas が同時に気づく——`ss-context.js` が同時呼び出しを1本の promise に畳む（畳まないと N 個の置換 device を要求して最後の1つしか残らない）。旧実装はこの復帰を**アトラスの PNG 再デコードごと canvas 数だけ**繰り返していた。
- **JS 露出は再設計後の ssplayer-webgl（`SSSurface` ＋ N 体の `SSPlayer`）と同型**（§0.2）。旧 `<ss-player>` の 1 canvas = 1 player・`play(name)` 1コールには揃えない——参照実装から既に消えているため。**選択と開始は別コール**（`setAnimation` + `play`）。
- **★テクスチャ供給は `ctx.addTextureFromBytes(hash, bytes)` が主経路**（✅ 実装済）。**device スコープなので1回で全 canvas・全 player に効く**（だから surface でも player でもなく context にある）。エンコード済みバイト列を `createImageBitmap(blob, {premultiplyAlpha:'premultiply'})` に通し、`copy_external_image_to_texture` で直接テクスチャへ入れて `set_texture` で登録する（CPU premul も 16 MB の一時コピーも通らない）。**両形態がここに集まる**:
  - embedded → **`player.embeddedTextures(pack?)`** = `{ hash, bytes }[]`（ssplayer-core の `extractEmbeddedTextures` と同型）。fetch は不要。
  - reference → **`player.textureRefs(pack?)`** = `{ hash, path, embedded }[]`（同 `extractTexturePaths`）。`embedded` が false のエントリを `new URL(path, ssabUrl)` で fetch して渡す。
- **★`player.load(src)` が最短形（✅ 実装済）**——`ssplayer-core` の `load(ssabUrl)` に相当。**URL 1つで2つの問いが同時に決まる**（バイト列は何か／兄弟は何を基準に解決するか）ので、これが既定の入口。パック名の上書きが要る場合だけ `loadSsab` を使う。
  - **中身は Rust の `SSPlayer::load_with` そのもの**（§9）。この面が足すのは `AssetLoader` の実装1つだけで、走査は3面で共有している。
  - **★base はローダが持ち、走査が順序で守る**: `load(src)` は `base_url: None`（＝document 基準）でローダを作る。`src` を**読んだあとに**走査が `set_base(src)` を呼び、そこで `.ssab` 自身の URL が base になる——URL 解決は base の最終セグメントを置き換えるので、これで隣の `atlas.png` も `../` もクエリ文字列も仕様どおりに解決する。**構築時に `.ssab` の URL を base にしてはいけない**（`src` が自分自身に対して解決されて `assets/assets/Basic.ssab` を取りに行く。**一度これで壊した**）が、順序がそれを構造的に防ぐようになったので、これは注意ではなく不変条件になった。`set_base` はホストが渡した `baseUrl`（`origin_url`）から解決し、前回の base に畳み込まないので、同じローダで2回ロードしても `assets/assets/` に潜っていかない——戻す側が `reset_base`。`loadSsab` は `src` が無いので `baseUrl` がそのまま base（相対で渡されても document に対して1回絶対化される。絶対 URL なら document を参照しない）。
- **★`player.loadSsab(bytes, pack?, baseUrl?)` がテクスチャまでロードする（✅ 実装済）**——`SS6PlayerForGodot` の `setSSABResource` と同じ形。**テクスチャロードは初期化に属し、アニメーション選択には属さない**（移植ガイド `30_core/10_initialization.ja.md` §3）。`setAnimation` は**同期のまま**で、モーション切り替えのたびに `Promise` を挟まない——これが置き場所を決める理由。
  - `async` なのはブラウザ側の事情だけ（Godot の `ResourceLoader::load` は同期、`fetch` / `createImageBitmap` は非同期）。**非同期はこの1箇所に閉じ込める。**
  - `pack` を省略すると `.ssab` 自身の名前を使う。**これが既定であるべき**——`ref_anime_pack`（§7.1）はエクスポータが書いた文字列で引くので、ホストが別名を付けると外部インスタンス参照が黙って解決されなくなる。
  - `baseUrl` は `.ssab` の取得元。reference 形態のアトラスパスはこれを基準に解決する（既定は document の URL）。移植ガイド `20_design/10_preparation.ja.md`「ベースディレクトリの保持」。
  - fetch は **`Promise.allSettled` で並列・ファイル単位**（hash 単位だと同一画像を2回落とす。上の §8.1 参照）。失敗は console 警告のうえ**続行**——`_load_external_ssabs` の `ERR_PRINT`＋`continue`、`PackResolver` の `console.warn`＋`continue` と同じ割り切り。
  - **外部インスタンス参照も同じ1コールで解決する**（§7.1）。`addSsab` は同形のまま残すが、位置づけは **URL で辿れないパック**用（ドロップ・バンドル・アーカイブ）——`ssplayer-core` が `InstancePackProvider` フックでカバーしている領域。
- **`ctx.hasTexture(hash)`** が「登録済みが勝つ」をホスト側から効かせるためのもの。`loadSsab` は自分でこれを見てから落としに行くので、**N 体で1回**は保たれる（Player 内部の flush が `decode_embedded_where` の述語で同じことをするのと同じ判定）。
- **`ctx.addTextureFromBitmap(hash, bitmap)`** は自前のアセットパイプラインを持つホスト向け（`set_texture` / WebGL 版 `bindTextureSource` に相当）。**device 喪失復帰は供給側の責務**。対して `addTextureFromBytes` は**エンコード済みバイト列を保持**するので `restoreDevice` が自分で復元する（ピクセルではなくエンコード済みなので 2048² で 16 MB ではなく数百 KB。WebGL 版が `{kind:'bytes'}` を retain するのと同じ判断）。
- **`ctx.addTexture(hash, rgba, w, h)`**（生 RGBA）は残置。CPU premul と `Retention::Retain` を通るので、エンコード済み画像を持っているなら `addTextureFromBytes` のほうが速く小さい。
- **`surface.stats()`** がフレームカウンタを JS オブジェクトで返す（players / prims / drawCalls / mergeableDraws / redundantTextureBinds / uploadedBytes ＋ `timestamp-query` があれば `gpuPassTimeNs`）。**§14.2 は native でしか測っていない**——ブラウザで同じ数字を取る手段がこれ。`examples/web/index.html` の②が毎フレーム表示する。
- `<ss-player-webgpu>` カスタム要素は「1要素 = 1 canvas = 1 player」の**便利ラッパ**として残す（広告のような単体用途はこれが最短）。ただし内部では上記の共有 context から surface を1枚取るだけなので、要素を何個置いても device は1つ。
- ssab は embedded / reference 両対応（§8.1）。**embedded のデコードは `createImageBitmap` ＋ `copy_external_image_to_texture` に寄せ、`image` crate を wasm から外した** ✅（`web` feature は `embedded` を引かない。brotli −26.9%、§8.1 に計測値）。**残課題**: web 面はテストが1つも無い。**ローダに何が渡されるか**は native で固定してある（`tests/loader_contract.rs`。`src` は verbatim、そのあと `set_base(src)` が1回、以降は裸の名前、という契約）が、**渡されたものをどう扱うか**——`fetch` / `createImageBitmap` / `copy_external_image_to_texture`——はブラウザ実機の目視が唯一の検証。`wasm-bindgen-test` ＋ CI のヘッドレスブラウザで埋める案は、WebGPU にフラグが要り CI が重くなるため**見送り**（2026-07-31）。
- ビルド/パッケージは `scripts/release-web.sh` 相当を追加し `pkg/ssplayer-wgpu-web/` を吐く。◻ 未着手。
- 注意: WebGPU 未対応ブラウザへのフォールバック方針（WebGL 版へ退避 or エラー表示）を Web Component 層で定義（§14）。現状は `sharedContext()` が非対応を検知して reject し、要素がエラー表示へ落ちるところまで。

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
int32_t      ss_scene_render(ss_scene_t*);                    // swapchain 取得 → 全 player を1 pass → present
void         ss_scene_resize(ss_scene_t*, uint32_t w, uint32_t h);  // ★ピクセル単位
int32_t      ss_scene_add_texture(ss_scene_t*, uint32_t hash, const uint8_t* rgba, uint32_t w, uint32_t h);  // reference 用（renderer 共有）
int32_t      ss_scene_add_texture_png(ss_scene_t*, uint32_t hash, const uint8_t* data, size_t len);  // ★同上・PNG のまま渡す
uint32_t     ss_scene_player_count(ss_scene_t*);              // 診断

ss_player_t* ss_scene_create_player(ss_scene_t*);             // ★1シーンに何体でも
void         ss_player_destroy(ss_player_t*);                 // シーンから外す（ハンドル自体は scene 破棄まで生存）
int32_t      ss_player_add_ssab(ss_player_t*, const char* pack, const uint8_t* data, size_t len);   // 外部参照解決用に全 pack 登録
int32_t      ss_player_load_ssab(ss_player_t*, const char* pack, const uint8_t* data, size_t len);  // 主 pack。embedded/reference 両対応
int32_t      ss_player_set_animation(ss_player_t*, const char* anim_name);   // 選択（確保はここだけ）
void         ss_player_play(ss_player_t*);                    // 開始 / pause からの再開
void         ss_player_pause(ss_player_t*);  void ss_player_stop(ss_player_t*);
void         ss_player_set_speed(ss_player_t*, float speed);  bool ss_player_is_playing(ss_player_t*);
void         ss_player_set_transform(ss_player_t*, float x, float y, float sx, float sy, float rot);
void         ss_player_set_order(ss_player_t*, int32_t order);       // 小さいほど先に描く
bool         ss_scene_is_device_lost(ss_scene_t*);            // デバイス喪失（§14）
int32_t      ss_scene_restore_device(ss_scene_t*);            // 喪失からの復帰。配下の player ごと戻る
void         ss_player_set_visible(ss_player_t*, bool visible);
void         ss_player_set_alpha(ss_player_t*, float alpha);
uint32_t     ss_player_animation_count(ss_player_t*);
const char*  ss_player_animation_name(ss_player_t*, uint32_t index);  // 次の呼び出しまで有効
uint32_t     ss_player_texture_count(ss_player_t*, const char* pack); // ★pack=NULL で主パック
int32_t      ss_player_texture_at(ss_player_t*, const char* pack, uint32_t index, ss_texture_ref_t* out);  // ★hash / path / embedded
int32_t      ss_player_canvas_size(ss_player_t*, float* w, float* h);
const char*  ss_last_error(void);                             // スレッドローカル。次の失敗まで有効
```

- **reference 形態の供給は「パックに聞いてホストが読む」** ✅: `ss_player_texture_at` が `{ hash, path, embedded }` を返し（`path` は `.ssab` 自身からの相対、§8.2）、ホストは `<.ssab を読んだディレクトリ>/<path>` を読んで `ss_scene_add_texture_png` に渡す。`embedded` が true のエントリは何もしない。**デコードをライブラリ側に用意した**のは、embedded 用に `image`（PNG）を既にリンクしているから——C ホストが ImageIO / WIC / libpng を各自抱える必要をなくす。ファイルを開くのは飽くまでホスト（ライブラリは wasm 面と共通で fs を持たない）。生 RGBA を持っているホストは従来どおり `ss_scene_add_texture`。

- **ハンドルの寿命（★実装済の形）**: `ss_player_t*` は「どの scene の、どの slot か」を持つ小さな box。**scene の Vec への生ポインタではない**——Vec は伸びると移動し、slot は再利用されるため。
  - `ss_player_destroy` は **box を解放しない**（scene が破棄されるまで生かす）。おかげで**二重 destroy も、破棄後の呼び出しも安全な no-op** になる。よくある C の間違いを未定義動作にしない。
  - slot は再利用するが、**世代番号**（`SlotId { index, generation }`）が付いているので、解放済みの id が「その slot を引き継いだ別の player」を指すことはない（`roster.rs` の `Roster<T>`。ウィンドウ無しで単体テストできるようホストから切り出してあり、**同じ問題を持つ web 面（§10）が canvas と player の両方に使い回している**）。
  - **シーンを破棄したら配下の player ハンドルは全て無効**（そこだけは use-after-free なので C 側で持ち越さない）。

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
| **P1 ssab 両対応 + WebGPU（② js）** ✅ | **ssab embedded/reference 両ロード（§8.1）** ＋ **WebGPU/js**：`ssplayer-wgpu` の `web` feature（wasm-bindgen、`libs/ssplayer-wgpu/src/web.rs` の `SsPlayerWeb` が canvas から wgpu を所有し借用モデル SSPlayer を駆動）、`examples/web` の `<ss-player-webgpu>` カスタム要素（ビルドは `scripts/build.sh platform=web`）。別 crate は作らず単一クレート。**wasm ビルド要件**: `web` feature が `ssruntime/std` を有効化（cargo は依存の全 crate-type を build するため、ssruntime の no_std cdylib/staticlib が wasm でアロケータ/パニック不足になる。std ならその衝突が消える。`ssruntime/panic-handler` は player の std と `panic_impl` 重複で不可）。release wasm-opt 用に `[package.metadata.wasm-pack.profile.release]` の SIMD 等フラグを player crate にも付与 | **ブラウザ実機で WebGPU 描画確認済**（P1 当時の release wasm 537KB。現在値は §14.1） |
| **P2 描画パリティ①** ✅ | varg 追加（PartColor blend 由来）、BlendType 全12種のパイプラインマトリクス、premul 厳密化（upload premul ＋ 逆/再premul シェーダ ＋ premul ブレンド係数） | PartsColor / BlendMode / Basic / Effect を PNG 目視で確認。既存フィクスチャのピクセル数は不変。WebGL 版と色・ブレンド一致 |
| **P3 マスク** ✅ | stencil 付きターゲット（config opt-in）、MaskPlanner 移植（`libs/ssplayer-wgpu/src/mask.rs`）、write/test パイプライン、pure マスク色スキップ、インスタンス継承、**Shape 幾何のマスク writer ＋ Shape content 描画**、白フォールバック初期化（§6） | マスク/Shape アニメが描画（Mask.ssab / Shape.ssab をヘッドレス GPU readback ＋ 実機で確認） |
| **P4 エフェクト/インスタンス** ✅ | **P0 の次に先行実施**。①内部を `begin_frame`/`append_batch`/`append_effect`/`end_frame` の累積 API＋transform へ（Stage 1）②インスタンス木＝子ごと独立 Context（Stage 2, §7.1）③エフェクト＝EffectSlot/EffectDrawPlan（Stage 3, §7.2） | エフェクト/インスタンスが描画される（実機確認済） |
| **P5 ネイティブ C-API + C++ viewer（③ ffi）** ✅（Metal） | `ffi` feature ＋ cbindgen（`build.rs`/`cbindgen.toml` → `include/ssplayer_wgpu.h`）、`host.rs` の `NativeHost`（生ハンドル→wgpu 所有）＋ `ffi.rs`（ABI 変換のみ）、`examples/viewer-metal-cpp`（Cocoa/Objective-C++、CMake が cargo を駆動）。再生 API は `set_animation` + 引数なし `play`（§9.3）。△ `viewer-dx12-cpp`（Windows・Win32/C++、metal 版の対訳として実装済だが**実機未検証**）／◻ 残: `viewer-vulkan-cpp`（Linux）／P7 で `ss_scene_*` へ作り直し | **macOS 実機で Metal 描画確認済**（Basic / Mask を C-API 経由で描画）。DX12/Vulkan は各実機で要確認 |
| **P6 統合シェーダ13種** ✅ | ss_shader 全13種（sepia/hsb/step/pix/noise/bmask/outline/blur/wave/move/scatter/circle/spot）を WGSL 移植（`internal.rs` `SS_SHADER_WGSL`）。パーツ属性 SHDR を per-part ディスパッチ：`(id_hash, blend)` 毎に**遅延生成**するシェーダパイプライン＋group(2) の動的オフセット uniform（param0..7 / cell_rect / pixel_size）。各シェーダは straight-alpha の Pixel を計算し `ss_finish`（＝P2 の PartsColor tail）で合成（blur のみ例外）。`ssTex` は逆premul＋`textureSampleLevel`（ループ/discard 後のサンプル可）。**制限（WebGL v1 同）**: map0/map1 未対応・shader×マスク非結合・Normal パーツのみ | Shader.ssab をヘッドレス GPU＋PNG 目視（13種の効果を確認）。shader 無しアニメはコスト0 |
| **P7 シーン再設計（複数プレイヤー）** ✅（7-1〜7-7・7-9。**7-8 は計測により保留**） | 下表。`SSInternalPlayer` を `SSRenderer`(device) / `SSScene`(ターゲット) へ解体し、`SSPlayer` から GPU リソースを剥がす（§1.2）。**破壊的変更・後方互換は取らない**（プロト段階） | 1つの描画ターゲットに複数体を正しく描ける（マスク持ち同士を含む）✅。web は device 1個で N 要素 ✅。C は 1 window に N 体 ✅ |
| （任意）Bevy アダプタ | `ssplayer-wgpu-bevy`（借用モデルの一利用例）。コアに依存を入れず別立て。**前提だった `set_texture` 借用チャネル（§8.2）は P7-5 で入った**ので着手可能 | Bevy の render graph で `SSScene` が描画。テクスチャは `GpuImage` を借用（二重アップロードなし） |
| **P8（候補）WebGL 由来の設計の解消** 🚧 | **§14.2 の棚卸し 10 件**（**10 = 計測 ✅ 実装済**）。P7-6 が頂点経路に対してやったことを、マスク / ブレンド / draw 発行に対しても行う（**並列化＝項目6 は実装・計測のうえ撤去**） | 項目ごとに個別。共通の完了条件は「`golden_corpus`（§13）が変更前とビット一致、または差分が意図どおりと説明できる」 |
| （後回し）Text / Nines | 旧版も未実装 | 必要時 |
| **（リリース）配布整備** 🚧 | 詳細は §12.2。配布面は **web（npm、`pkg/`）** と **desktop FFI（cdylib/staticlib ＋ ヘッダ）** の2つ。`release.yml` が無く、CI も Linux / Windows / Web だけで **macOS ジョブが無い**。macOS の cdylib は署名＋公証が要る | tag から各面が自動で出る。macOS 成果物が公証済み（`notarytool` が `Accepted` を返す） |

### 12.1 P7 の分解と依存関係

```
[7-1] ステンシル完全巻き戻し ──┐   ← 単独で入る。何をするにも先に
[7-2] transform/order/visible ─┤   ← 単独で入る。配管は既存（§5.4）
[7-3] SSInternalPlayer 解体 → SSRenderer(device) + フレーム蓄積
                               ↓
[7-4] SSScene（アリーナ / projection / order 合成 / restore の単位）
                               ↓
        ┌──────────────────────┼───────────┬──────────┐
[7-5] pack 共有 + set_texture   [7-6] SoA   [7-7] C-API  [7-9] web 面
      借用チャネル（§8.2）      パススルー  作り直し     作り直し
                                （§5.5）   （ss_scene_*） （SsContextWeb, §10）
                                    ↓
                           [7-8] draw マージ ← ★計測により保留（§14.2）
```

**7-7 と 7-9 は同じ作り直しの2面**——「ホストが device と renderer を1つ持ち、その下に N 個のターゲット／N 体の player がぶら下がる」形にするだけで、違うのはウィンドウの取り方だけ。世代付き slot（`roster.rs`）を共有しているのはそのため。

| # | 内容 | 参照 | 単独で入るか |
|---|---|---|---|
| 7-1 ✅ **実装済** | ステンシル完全巻き戻し（不変条件）。`player.rs` の `unwind_masks` | §6 | 単独で入った |
| 7-2 ✅ **実装済** | `set_transform` / `reset_transform` / `set_order` / `set_visible` / `set_alpha`。`order` は保持のみ（消費するのは 7-4 の `SSScene`）。`tint` は §5.5 後に回す——PartsColor の Mix 式より後段に掛ける必要があり、頂点色への畳み込みでは表現できない | §5.4 / §9.3 | 単独で入った |
| 7-3 ✅ **実装済** | パイプライン遅延生成（39本 eager → 使ったぶんだけ）＋ `SSRenderer` 分離。ホストが所有し `update`/`draw` で貸す。テクスチャ・pipeline・layout・sampler・白フォールバックが device 単位の共有に。**ファイルはまだ `internal.rs`**——`renderer.rs` / `scene.rs` への分割は 7-4 と同時に1回で行う（2度のファイル移動を避けるため） | §5.2 / §9.1 | 遅延生成は単独で先行 |
| 7-4 ✅ **実装済** | `SSScene`（旧 `SSInternalPlayer` を改名）。1 シーンが N プレイヤーを `order` 昇順で1フレームに積む。`SSPlayer` から **GPU 型のフィールドが完全に消えた**（`new()` は引数なし）。投影・アリーナ・復帰はシーン側へ。`internal.rs` を `renderer.rs` / `scene.rs` へ分割済 | §9.2 | 7-3 の後 |
| 7-5 ✅ **実装済** | `SSPack`（`pack.rs`、CPU のみ・`Arc` 共有・パック名は ssab 由来）＋ `set_texture` / `forget_texture` 借用チャネル ＋ per-texture `AlphaMode`（group(1) の `TexInfo` uniform。テクスチャサイズも同居させ §5.5 の器を先に作った）。埋め込みアトラスは**登録済みならデコードごとスキップ**するので N 体で1回 | §8.2 | 7-3 の後 |
| 7-6 ✅ **実装済** | SoA パススルー（GPU 頂点展開）。**頂点バッファ / index buffer を撤去**し、group(0) を「シーン」（投影 ＋ `f_arena` ＋ `prims` ＋ `mesh_stream`）に。幾何4種＝頂点エントリ4種（`vs_quad`/`vs_mesh`/`vs_shape`/`vs_effect`）でパイプラインキーに `geom_class` が増えた。**フィクスチャ134枚がビット完全一致**（§13） | §5.5 | 7-4 の後 |
| 7-7 ✅ **実装済** | C-API を `ss_scene_*`（ウィンドウ）+ `ss_player_*`（1体）の**2ハンドル**へ。`NativeHost` が世代付き slot（`roster.rs`）で N 体を持ち、`ss_scene_create_player` / `ss_player_destroy` / `set_transform` / `set_order` / `set_visible` / `set_alpha` を追加。テクスチャ供給は scene 側（`ss_scene_add_texture`）——テクスチャマップが device スコープだから。Metal viewer は**既定で2体**表示し `+/-` で増減 | §11 | 7-4 の後 |
| 7-9 ✅ **実装済** | web 面を `SsContextWeb`（ページ）/ `SsSurfaceWeb`（canvas）/ `SsPlayerWeb`（体）の3オブジェクトへ。**adapter/device/`SSRenderer` はページで1つ**、canvas ごとに `SSScene`、1 canvas に N 体。喪失復帰もページで1回。`setAnimation` + `play` へ分離し、`surface.stats()` で web 側の計測器を用意（§14.2）。7-7 と `roster.rs` を共有 | §10 | 7-4 の後 |
| 7-8 ⏸ **保留（計測）** | draw_call マージ、マスクのランレングス結合（**indexless 化は 7-6 で完了**——index buffer 自体が無くなった）。**上限を測ったら 256体で 0.03 ms**（`mergeable_draws` が 31%、encode 自体が 0.09 ms）。ステンシル境界と `shader_slot` を跨げない条件付き実装になるので、§14.2 が項目 7 / 4 を見送ったのと同じ理由で見送る | §5.5 / §6 / §14.2 | — |

**単独で先に入る小改善**（P7 を待たない）: 7-1 / 7-2 / パイプライン遅延生成 / マスク時のランレングス結合 / `position.z` 削除（§5.1） / ~~premul スクラッチ再利用（§8）~~ → **バッファを受け取って in-place premul する形で解消済み**（§8 / §14.2 item 9）。

### 12.2 配布・リリース整備（未着手）

公開前に要る作業。描画機能とは独立なので P7 / P8 の進行を待たない。

**配布面は2つ**。crate 自体の crates.io 公開は現状の配布面に入れていない。

- **web**: `scripts/build.sh platform=web` が wasm-pack 出力を `pkg/` に吐く。`pkg/` は `.gitignore` が `*` のビルド生成物（追跡していない）で、`pkg/package.json` は name `ssplayer-wgpu` / version `0.1.0`。npm publish の配線は未着手。
- **desktop FFI**: `build.sh` が `bin/libs/<platform>/<arch>/` に cdylib / staticlib を集め、ヘッダは `build.rs` の cbindgen が `include/ssplayer_wgpu.h` に吐く。**host ビルドのみでクロスコンパイル非対応**なので、プラットフォームごとにランナーが要る。

**crates.io は SDK 待ち**: `ssruntime` / `ssab` は submodule への path 依存（§2）。cargo は path のみの依存を持つ crate を publish できないので、`ssplayer-wgpu` を crates.io に出すなら SDK 側の crates.io 公開が先になる。

**macOS の署名・公証**: cdylib（`libssplayer_wgpu.dylib`）は他 Player と同じ理由で署名＋公証が要る——配布 zip をブラウザで落とすと `com.apple.quarantine` が付き、Gatekeeper が未公証コードの `dlopen` を拒むため。staticlib（`.a`）は公証不可・不要（リンク先のバイナリを消費側が署名する）。配線は SpriteStudio-SDK #355 / SSPlayerForGodot #264 をそのまま移植でき、組織 secret（`APPLE_DEV_ID_APP_*` / `ASC_API_*`）も既存のものを使う。**ただし現在の CI に macOS ジョブが無い**ので、まずそこから。`examples/viewer-metal-cpp` も CI では一度もビルドされていない。

**版と SDK の紐付け**: 版の stamping が無い（crate と `pkg/package.json` の `0.1.0` は手書き）。かつ SDK を**リリース zip ではなく submodule** で取り込んでいるため、他 Player の `scripts/SDK_VERSION.txt` 方式が使えない。成果物には**ビルド元の SDK commit** を記録する必要がある。

**不足しているリポジトリ資材**: `CHANGELOG.md` / `THIRD_PARTY_NOTICES.md` / 成果物の `SHA256SUMS`。いずれも他 Player は持っている。

---

## §13 検証

| 対象 | 手段 |
|---|---|
| Rust 描画（開発） | `examples/viewer/`（winit+wgpu、`assets/overall/` 再生。横=ssab切替/縦=アニメ切替）。**`.ssab`／フォルダの D&D で任意のパックを再生**——アトラスは落とし元ディレクトリを基準に `SSPack::textures()` のパスで解決する（§8.2） |
| インスタンス/エフェクト（ヘッドレス） | `libs/ssplayer-wgpu/tests/instance.rs`（probe。frame 推移・quad 生成を GUI 抜きで確認） |
| マスク/Shape（ヘッドレス） | `libs/ssplayer-wgpu/tests/mask.rs`（planner probe：writer 分類・Shape-mask）＋ `libs/ssplayer-wgpu/tests/render.rs`（GPU offscreen readback：masking on/off 差分・Shape content 描画量。adapter 無ければ skip。PNG 保存で目視も可） |
| 統合シェーダ（ヘッドレス） | `libs/ssplayer-wgpu/tests/render.rs::render_integration_shaders`（Shader.ssab を描画し、SS WGSL コンパイル＋per-part ディスパッチ＋params で描画量が出ることを確認。adapter 無ければ skip） |
| WebGPU（主） | `examples/web/`（ブラウザで①`<ss-player-webgpu>`＋②1 canvas に N 体）。**②が要点**——device がページで1つであること・体を増やしてもアップロードが増えないこと・`stats()` の数字を、ブラウザでしか確認できない。初回起動計測（取得→wasm初期化→初フレーム ms） |
| C-API（③ ffi） | `cargo build -p ssplayer-wgpu --features ffi`（cbindgen がヘッダを再生成。**ネイティブの他コマンドではこの面はコンパイルされない**——web feature と同じ死角）＋ **`cargo test -p ssplayer-wgpu --features ffi --lib`**（`roster.rs` 単体テスト：破棄した id が後継を指さないこと、二重 destroy が no-op であること。ハンドル寿命は C / JS 双方の未定義動作・取り違えに直結するので、ここだけは死角に置かない。`--features web` でも同じテストが走る） |
| DX12（副） | `examples/viewer-dx12-cpp/`（Windows 実機、backend=DX12 強制）— viewer は実装済、実機での実行が未了 |
| Metal（副） | `examples/viewer-metal-cpp/`（macOS 実機、backend=Metal 強制）。`cmake -B build && cmake --build build && ./build/ssviewer-metal [asset-dir] [pack]`。**`.ssab`／フォルダの D&D で任意のパックを再生**——embedded / reference 両対応で、reference のアトラスは `ss_player_texture_at` が返すパスを落とし元ディレクトリと結合して `ss_scene_add_texture_png` に渡す（§8.2）。ドロップ経路は C 面の reference 供給を実機で通す唯一の検証でもある |
| Vulkan（将来） | `examples/viewer-vulkan-cpp/`（Linux 実機、backend=Vulkan 強制） |
| **複数プレイヤー（ヘッドレス）** ✅ P7-1/7-2/7-3 | `libs/ssplayer-wgpu/tests/scene.rs`。**レンダラ共有**は `players_share_one_renderer`（同一 pack の2体目でパイプライン本数が増えないこと）と `pipelines_are_built_on_demand`（構築直後は0本、Basic.ssab で1本、同一フレーム再構築で増えない＝キーが安定）で見る。**ステンシル残留の回帰**は「1 pass に N 体」と「1 体 1 pass（stencil を毎回クリア）」の**ピクセル完全一致**で検出する——後者は構造的に漏れようがないので基準になる。マスク持ち同士（`Mask.ssab` ×2）と混在（`Mask` × `Basic`、順序両方向）。`set_transform` は**同一フレームで**（`update(0.0)` で再構築）整数ピクセル平行移動し、元画像を N px ずらしたものと完全一致することを要求。`set_visible(false)` は描画ゼロ、`set_alpha(0.0)` も描画ゼロ、0.5 はその中間。`order` の並べ替えは `order_decides_who_draws_on_top`（前後入替で composite が変わる ＋ スライス順を逆にしても値順に描く） |
| **1 レンダラに N ターゲット（ヘッドレス）** ✅ P7-9 | `tests/pack.rs::scenes_share_one_renderer`。web 面が乗っている土台——「レンダラは device スコープでフォーマット非依存」——を native で押さえる。**2枚目の scene はわざと別のカラーフォーマット**（canvas はブラウザの好むフォーマットを取るため）にして、フォーマットが scene のパイプラインキー側にあり renderer 側に無いことを固定する。判定は `texture_upload_count()` が増えないこと ＋ `pipeline_count()` は**増える**こと（増えなければ2枚目が正しく描けているはずがない）＋ 2枚目の後も1枚目が描けること |
| **pack 共有 / 借用テクスチャ（ヘッドレス）** ✅ P7-5 | `libs/ssplayer-wgpu/tests/pack.rs`。**同一 pack N 体でアップロード1回**は `texture_upload_count()` で見る（`texture_count()` は N 回上書きしても同じ値なので判定にならない）。`set_texture` は同じピクセルで `upload_texture` と**ピクセル完全一致**、`forget_texture` で白フォールバックに落ちること。**`AlphaMode` は合成アトラス（アルファ横ランプ）で検証する**——出荷フィクスチャはほぼ不透明で、premul と straight が一致してしまい**フラグを無視しても通ってしまう**。ランプなら誤申告は 1/α ぶん明るくなり、正しい申告 0.001 に対し 11.6 と桁で開く |
| **全フィクスチャ回帰（ヘッドレス）** ✅ P7-6 | `libs/ssplayer-wgpu/tests/golden.rs::golden_corpus`（`#[ignore]`）。全パック × 先頭4アニメ × 4フレーム × stencil on/off ＋ 2体構成 = 134 枚を readback し、**変更前のダンプとバイト比較**する。`SS_GOLDEN_DIR=<出力> [SS_GOLDEN_COMPARE=<基準>] cargo test --test golden -- --ignored`。<br>他のレンダリングテストは「描画量」と「特定の不変条件」しか見ないので、**パーツが1px ずれた／色がわずかに変わった**を誰も検出できない——そこを埋めるのがこれ。GPU/ドライバ依存なので基準は**同じマシンで取る**（チェックインしない）。P7-6 の頂点パス総入替はこれで 134/134 ビット一致を確認した |
| **幾何4クラス（ヘッドレス）** ✅ P7-6 | `tests/scene.rs::every_geometry_class_draws`。Basic/Mesh/Shape/Effect を1本ずつ描き、各クラスの頂点ステージが**何かを出す**ことだけを下限で見る（正確な出力は `golden_corpus` の担当）。頂点エントリが4つに割れた以上、1つ死んでも他は無傷という壊れ方をするため |
| **複数プレイヤー（目視）** ✅ | `examples/viewer` が **1 パスに N 体**（既定2体、`+/-` で 1〜8）。要点は**重ねられること**——`L` で Row / Stack（中央に重ねる）を切り替え、`O` で描画順を反転する。相互干渉は重なりの上でしか起きないので、Row だけでは 1 体固定と同じものしか見えない。`Tab` の focus で**体ごとに別の `.ssab`／アニメ**を選べるので、`Mask` × `Basic` を重ねて「先の体のマスクが後の体をクリップしない」（`unwind_masks`、§6）が一目で分かる。`V` / `[` `]` は per-player の `set_visible` / `set_alpha`。パックは `SSPack` を1回パースして全体で共有する（§8.2）。ステータス欄は 0.5 秒ごとに `stats()` を出し直すので、体数を変えながら `players` / `prims` / `draw_calls` / GPU 時間の動きが読める |
| **体数スケール（計測）** ✅ P8-6 | `tests/scale.rs::frame_cost_by_player_count`（`#[ignore]`、**release で**）。1〜256体の `update` ＋ pass encode を**同一ループで**実測。**項目6（スレッド化）の撤去と項目5（render bundle）の却下はこの数字で決めた**。`update` の中身を変えたら再測定すること |
| **フレームコスト** ✅ P8-10 | `SSScene::stats()`（`tests/scene.rs::a_frame_reports_what_it_cost` / `gpu_pass_time_comes_back`）。viewer と Metal viewer が毎フレーム表示する。**§14.2 の 4〜9 に着手する前後で必ずここを比べること**——「速くなるはず」で入れた変更が実際どうだったかは、これ以外に判定材料が無い |
| **wasm サイズ** | `./scripts/build.sh platform=web`（**release**。`build=debug` は wasm-pack `--dev` = wasm-opt を飛ばすので測ってはいけない）→ `ls -l pkg/*.wasm pkg/*.js` ＋ `gzip -9 -c ... \| wc -c`。**比較対象の ssplayer-webgl も同一マシンの release で測る**（`SpriteStudio-SDK/pkg/runtime-web/release/`。`packages/ssplayer-core/wasm/` は debug のコピーが置かれていることがあり、踏むと 5 倍の値を掴む）。§14.1 の表を更新する |
| WebGL 版との一致 | 同一 ssab を両者で再生し目視/スクリーンショット比較（色・ブレンド・マスク）。**P7 以降は「複数体を置いた画」でも比較する**（ssplayer-webgl も surface+players 構成のため同条件） |
| **CI の GPU（ヘッドレス）** ✅ | runner に adapter が無いと、上の readback 系テストは**全部自分を skip する**——緑なのに一枚も描いていない（AGENTS.md「Beware green rendering tests」）。Linux job に **Mesa lavapipe**（`mesa-vulkan-drivers`、ICD は `VK_DRIVER_FILES` / `VK_ICD_FILENAMES` で lavapipe 1本に固定）を入れ、`vulkaninfo --summary` を**失敗させるステップとして**前に置いた（ドライバ導入が壊れたら緑にならない）。これで render / scene / pack / events / timing が CI で実際に走る。`golden_corpus` は同一マシンのダンプ比較なので対象外（`#[ignore]` のまま、ローカルの before/after ツール）。ソフトウェアラスタライザなので**速度は測れない**——`scale.rs` も `#[ignore]` のまま |
| Rust 品質 | `cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --all -- --check` / `cargo test` |

---

## §14 既知の制約・要調査

- **premul の一貫性**が最重要。アップロード・シェーダ・ブレンド係数のどこか1つでも崩すと合成破綻。
- **premul と WebGPU の非対称（★解消済）**: WebGL は `UNPACK_PREMULTIPLY_ALPHA_WEBGL` でドライバが無料で premultiply するが、**WebGPU の `writeTexture` に等価は無い**——`upload_texture` は CPU で premul しており、2048² で 16 MB の一時コピーが出る。**web 面はこの経路を通らなくなった** ✅: `createImageBitmap(…, {premultiplyAlpha:'premultiply'})` ＋ `copy_external_image_to_texture`（dest `premultiplied_alpha: true`）で、デコードも premultiply もブラウザ側で済み、ピクセルは wasm メモリを一度も通らない（§8.1 / §10）。ネイティブの `upload_texture` は従来どおり。engine から借りるテクスチャは premul されていないのが普通なので `AlphaMode` フラグで受ける ✅ P7-5（§8.2）。なお `upload_texture` は **`AlphaMode::Straight` で上げれば CPU premul を省ける**が、そうしない——線形補間はアルファ境界で premultiplied のほうが正しく、フチが出るのを避けるため。省くのは供給側が既にその判断をしている `set_texture` 経路だけ。
- **stencil ターゲット** ✅ 実装済: `SSPlayerConfig.stencil_format`（P7 以降は `SceneConfig`）で opt-in し、ホストが毎 pass に attach（§6）。
- **INCR/INVERT 混在マスク**は単一ステンシルで近似（厳密解は coverage-bitmap、要検討）。
- **ステンシル残留** ✅ P7-1 で解消: `render_node` がフレーム末尾で未消化 writer を全て巻き戻す（`unwind_masks`）。これが無いと**1 pass に複数 Player を積んだとき A のマスクが B をクリップ**した。回帰は `tests/scene.rs::stencil_unwind_isolates_players`（§6 / §13）。
- **WebGPU 非対応環境のフォールバック**方針（WebGL 退避か）未定 → §14.1。
- **raw-window-handle の FFI 越え** ✅ 実装済: Rust トレイトは C に渡せないため、平坦な `ss_window_handle_t{ kind, screen, window, display }` を C-API に定義し、`ffi.rs` が `raw-window-handle` 型へ組み立てる（§11）。CAMetalLayer だけは rwh に表現が無いので wgpu の `SurfaceTargetUnsafe::CoreAnimationLayer` を使う（Apple ターゲットのみ）。**Win32/Xlib/Wayland の経路は実機未検証**——DX12/Vulkan viewer で確認する。
- **テクスチャ供給** ✅: embedded（`SSPack::decode_embedded`）/ reference（ホスト供給）/ 借用（`set_texture`）の3経路が `SSRenderer` の1つの hash マップに集約される（§8.1 / §8.2）。
- **★取りこぼしはリストで返る（✅ 実装済）**: `load` が `Err` を返すのは「`.ssab` 自身が読めない／壊れている」場合だけ——1枚欠けてもキャラごと落とさない（§8.1）——ので、**完全に揃ったのか、揃わないまま再生しているのか**を呼び出し側が区別できる必要がある。`SSPlayer::missing_assets() -> &[MissingAsset { kind, name, hash }]` が直前のロードで取れなかったものを順に返す（`kind` は `Texture` = 白シルエットで描かれる／`Pack` = そのインスタンスパーツが出ない）。**`AssetLoader::warn` は人間向け、こちらはプログラムが分岐するためのもの**という二本立て。C は `ss_player_missing_count` / `ss_player_missing_at`、js は `player.missingAssets()`。ロード毎にクリアされるので、プレイヤーの履歴ではなく直前の1コールを表す。検証は `tests/loader_contract.rs::a_partial_load_is_reported_without_failing` / `::a_missing_sub_pack_is_reported_by_name`。
  - **`warn` の行き先はまだホストに渡せない**（`eprintln!` / `console.warn` 固定）。プログラムからの判定は上で足りるので、ログ基盤に載せたくなったときに考える。
- **★パスで辿れないサブパックは provider で解決する（✅ 実装済）**: 外部インスタンス解決の既定は `<.ssab のディレクトリ>/<pack 名>.ssab`（§7.1）だが、アーカイブの中・engine のアセットサーバ・D&D で受け取ったバイト列など、その規則で辿れないパックがある。`AssetLoader::read_pack(name) -> Option<Vec<u8>>` が**名前で引く口**で、パス経由より**先に**試され、`None`（既定実装）ならパスへフォールバックする。`ssplayer-core` の `InstancePackProvider` と同じ位置づけ（あちらも fetch より先に provider を見る）。既定実装があるので、実装していないローダの挙動は一切変わらない。
  - **もう一つの逃げ道は事前登録**: 走査は `has_pack(name)` が真のパックを飛ばすので、`add_ssab` で先に入れておけば読みに行かない。ただしそれは「その `.ssab` がどのパックを参照しているか」を読む前に知っている必要があり、**走査に訊かせる**ための口が `read_pack`。
  - 検証: `tests/loader_contract.rs::a_provider_answers_for_packs_with_no_path`（provider が答えた分はファイルを探しに行かないことまで）／`::declining_the_provider_falls_back_to_the_path`／`::a_pre_registered_pack_is_not_looked_for`。
  - **C / js 面に provider は出していない**。あちらは `add_ssab` の事前登録で同じことができ、コールバックを ABI と JS 境界に通す価値が今は無いため。engine アダプタは `AssetLoader` を自分で実装するので、そちらが本来の利用者。
- **データ所有契約（§1.1）**: ssab は Player 所有→ssruntime へ borrow、FrameData/SoA は ssruntime 所有→Player が borrow。ssab を Context より先に破棄しない／FrameData 借用をフレーム跨ぎで保持しない。
- **★パックの resource 寿命（✅ 修正済）**: 上の「ssab を Context より先に破棄しない」が**同名再登録という経路で破れていた**。`build_node` は各 `Context` を pack の resource に**生ポインタ**で束ねる（`SsabResource::resource_static_ptr` の契約は「ポインタを使っている間、どこかの `Arc` を生かしておくこと」）が、その `Arc` を持っているのは Player の pack マップだけ。したがって**再生中の木が使っているパックを同名で再登録する**と（`add_pack` / `add_ssab`）`HashMap::insert` が旧 pack を落とし、木が解放済みの resource を評価し続ける = use-after-free になっていた。safe な Rust から到達し、`ss_player_add_ssab` / js の `addSsab` も同じ経路で、次の `update` がランタイム内部で落ちる。`set_pack` だけは `root = None` に救われていた。
  - 対処: `add_pack` は置換された `SSPack` を `retired` へ退避し、**木を作り直す `set_animation`**（と木を捨てる `set_pack`）で解放する。木が無い間は退避せずその場で落とす。
  - 同時に契約が確定した: **再登録は再生中の中身を差し替えない。** 木は組まれた時の resource を最後まで使い、新しいパックを見るのは次に組む木。差し替えたければ `set_animation` を呼ぶ。
  - 回帰は `tests/pack.rs::re_registering_a_pack_does_not_pull_it_out_from_under_the_tree`（無改修だとランタイム内で index out of bounds、修正後は「再登録しても無中断再生とピクセル一致」＋「作り直した木も一致」）。
- **エフェクト blend はコマンド毎**（batch.blend_type ではない）点を実装で取り違えない。
- **ss_shader 統合シェーダ13種** ✅ 実装済（`renderer.rs` `SS_SHADER_WGSL`、WebGL `SSShaderLib.ts` を WGSL 移植）。残課題: map0/map1 セカンダリテクスチャ・shader×マスク結合・GLSL 版との厳密ピクセル一致検証。
- **WebGL 版からの堅牢性 carry-over（借用モデル前提）** ✅ 実装済: WebGL 版 Player（`SSPlayerForWeb`）で実装済みの広告向け堅牢性を wgpu へ持ち込む対応表。Device/Queue/Surface は**ホスト所有**（§1.1 借用モデル）なので、喪失検知・再生成が**ホスト責務**になる点が WebGL 版との最大の差。
  - **デバイス/サーフェス喪失復帰** ✅ 実装済: **P7 でリソースがスコープ別に割れた（§1.2）ので、復帰も3段になった**——`SSRenderer::restore_device(&Device, &Queue)`（device 単位: pipeline / layout / sampler / 白 / テクスチャ。**1回で全 scene・全 player を覆う**）→ `SSScene::restore_device(&Device, &Queue)`（ターゲット単位: 投影 ＋ フレームバッファ群。記憶した viewport を貼り直す）→ `SSPlayer::restore_textures(&mut SSRenderer, &Device, &Queue)`（player 単位: 自分の embedded アトラスを再提供するだけ。**player は GPU リソースを持たないので再生成するものが無い**）。順序は逆にできない——player が登録し直す先のレンダラが、新デバイス上に先に存在している必要がある。WebGL 版 `SSWebGLRenderer.restoreContext()` ＋ `SSWebGLResourceManager.restoreContext()` に相当。再生状態（ロード済みパック・インスタンス木・現在フレーム）は CPU 側なので無傷——**中断したフレームから再開**する。テクスチャは**生成元を保持して再アップロード**（retained-source）: embedded は Player が所有する `.ssab` から**再デコード**するのでピクセル複製を持たず、ホスト供給分のみ `upload_texture` が**アップロードしたそのバッファ（premul 済み）**を保持する（複製を嫌うホストは `forget_texture_sources()` で破棄し自前で再供給）。**straight ではなく premul 済みを持つ**のは、保持のためだけに straight のコピーをもう1本取る理由が無いから——復帰は `write_and_register` で書き戻すだけで、premul を通し直さない（通せば半透明テクセルが二重に暗くなる）。straight を要求する経路はどこにも無い。**カラー/ステンシルのフォーマットが変わった場合は対象外**——フォーマットはパイプラインキーの一部（§5.2）なので `SSScene` を作り直すこと（`SSPlayer` は無関係。GPU リソースを持たないので作り直す意味が無い）。
    - web(§10): **`SsContextWeb`**（ページで1つ）が `Device::set_device_lost_callback`（wgpu の webgpu バックエンドが `GPUDevice.lost` を購読）で喪失を検知し、`isDeviceLost()` で公開。`restoreDevice()`（async）が adapter/device を再取得 → renderer → **各 canvas の surface / stencil / scene → 各 player のアトラス**、の順に復帰させる。**喪失は device のものなのでページで1回**——`ss-context.js` が N 個の canvas から同時に来る呼び出しを1本の promise に畳む（畳まないと置換 device を N 個要求して最後の1つしか残らない）。喪失中の `frame()` / `resize()` は GPU に触れない（`resize` はサイズのみ記録し、復帰時に反映）。
    - native(§11) ✅ 実装済: `SurfaceError::Lost/Outdated` は `NativeHost::render` が**サーフェス再構成**（resize と同経路）で吸収する。デバイスまるごとの喪失は **`NativeHost::restore_device()`**（`host.rs`、C は `ss_scene_restore_device`）が web 面と同じ順序で戻す。検知は `Device::set_device_lost_callback` ＋ `is_device_lost()` で、**喪失中は `update`/`render`/`resize` が GPU に触れない**（`resize` はサイズだけ記録し復帰時に反映）。置換アダプタが元のサーフェスフォーマットを提示できなければ `SS_ERROR_GPU`——そこは restore ではなく `ss_scene_create` からの作り直し。
      - **配線ごと `NativeHost` の中に置く**（アクセサ4本を生やす形は採らない）。復帰に要る `&Device`/`&Queue`/`&mut SSRenderer`/`&mut SSScene` を1つも公開していなかったため、以前ここにあった「必要になれば1メソッド呼ぶだけ」という先送りは**クレート外から実行不能**だった。wgpu 一式を自前で組むホストは `examples/viewer` の側で、`NativeHost` は「ウィンドウハンドルを渡せば全部やる」側。
      - **テストは無い**（ウィンドウハンドルが要るのでヘッドレスに置けない）。順序そのものは `tests/render.rs::restore_device_rebuilds_gpu_state` が層2 の側で押さえている。
    - 検証: `tests/render.rs::restore_device_rebuilds_gpu_state`（**2枚目の Device** へ復帰させ、無中断レンダーとピクセル一致を要求。`forget_texture_sources` 後は不一致になることを negative control として確認）。
  - **可視時のみ描画（pauseWhenHidden・web のみ）** ✅ 実装済: `<ss-player-webgpu>`（`examples/web/ss-player-webgpu.js`）が **IntersectionObserver ＋ visibilitychange** で画面外/裏タブ時に rAF ループを止める（WebGL 版 `SSSurface` の `_shouldRun` / `_syncTicker` と同構造。再開時は時計をリセットして待機時間ぶんの巨大 delta を避ける）。既定 ON、`pause-when-hidden="false"` で明示 opt-out。IntersectionObserver の初回コールバックまでは**非表示扱い**——画面外の初期フレームを描かない。native は winit の `Occluded`/`RedrawRequested` が相当。
  - **精度指定は非該当**: WGSL は f32 既定で `mediump/highp` の概念が無いため、WebGL 版の fragment highp 検出はポート不要。
  - **プレゼンテーション設定**: web は `context.configure({ alphaMode:'premultiplied', … })` ＋ `requestAdapter({ powerPreference:'low-power' })` が WebGL の `premultipliedAlpha` / `low-power` 相当。`desynchronized` / `failIfMajorPerformanceCaveat` に WebGPU 直接等価は無い。

### 14.1 WebGPU を選ぶ理由と、ssplayer-webgl を残す理由

**両者は置き換え関係ではない。** 棲み分けを明文化しておく（「WebGL 版はいずれ畳む」という前提を持たないこと）。

**WebGPU でしかできないこと（＝この Player の存在理由）**

| 能力 | WebGL 側 | この Player で何が変わるか |
|---|---|---|
| **compute / storage buffer** | WebGL2 に SSBO も compute も無い（**原理的に不可**） | ✅ SoA パススルー（§5.5）。CPU の頂点組み立てが消え、体数・パーツ数の天井が上がる |
| **同一 device を engine と共有** | GL は global state の奪い合いで実質不可能 | **本命**。Slint / Bevy と1 device で合成できる（借用モデルはこのためにある） |
| **native と同一コード** | web 専用 | Metal / Vulkan / DX12（`examples/viewer-metal-cpp` 実績あり） |
| render bundle / indirect draw / timestamp query | 無し（`EXT_disjoint_timer_query` は Spectre 対策で実質消滅） | draw_call 再 encode の削減、GPU 側カリング、バッチ単位の実測 |
| 安いオフスクリーン＋リードバック | `readPixels` は同期ストール | group alpha の厳密実装、テストのピクセル比較、サムネイル生成 |

**それでも WebGL を選ぶ理由（弱み）**

1. **到達率。これが決定打であり、事実上これだけ。** Safari は 26（2025）、Firefox は 141（Windows・2025）でようやく。**iOS 26 未満には無い**。広告クリエイティブ・教材のような「全端末で必ず動く」用途は WebGL2/1 一択（正確な数字は caniuse で都度確認）。
2. **初期表示レイテンシ。** adapter → device が2段の Promise、加えて現行はパイプライン39本を起動時に全コンパイル（→ P7 で遅延生成、§5.2）。WebGL 版は program 1〜2本を遅延コンパイル。First Paint が KPI の用途では効く。
3. **フォールバックの二重保守。** 非対応なら別レンダラが要る＝ WebGL 版は結局残る。
4. **canvas 周りの機能差**（`desynchronized` / `failIfMajorPerformanceCaveat` / `preserveDrawingBuffer` に等価なし）、**premul の非対称**（§8）、**ツールの成熟度**（Spector.js 相当が弱い）。

**バイナリサイズは弱点ではない（★2026-08-05 実測に更新）**

両者とも **release ビルドを同一マシンで**測った値。ssplayer-webgl 側は `SpriteStudio-SDK/pkg/runtime-web/**release**/ssruntime_bg.wasm` と `packages/ssplayer-webgl/dist/browser/ss-player.element.min.js`。

|  | ssplayer-wgpu (WebGPU) | ssplayer-webgl | 比 |
|---|---|---|---|
| wasm | 580,700 B | 277,601 B（`ssruntime_bg.wasm`） | 2.09× |
| JS | 125,867 B（wasm-bindgen glue） | 307,415 B（`ss-player.element.min.js`） | 0.41× |
| **raw 合計** | **706,567 B** | **585,016 B** | **1.21×** |
| **gzip 合計** | **233,129 B** | **146,864 B** | **1.59×** |

wgpu の WebGPU バックエンドは WGSL をブラウザへそのまま渡すので **naga のバックエンド群が丸ごと落ちる**のが効いている。

**wasm 単体の 2.09× を見て焦らないこと。両者は同じものを wasm に入れていない**——ssplayer-webgl の wasm は ssruntime だけで、**描画は 307 KB の JS 側**にある。wgpu 版は runtime も描画も wasm に入っている。だから比較に意味があるのは raw / gzip の**合計**行だけで、そこでは 1.21× / 1.59×。**転送量（gzip 合計）は前回計測の 264,144 B → 233,129 B（−11.7%）、対 webgl 比も 1.79× → 1.59× と改善している**。`image` を外したぶん（§8.1）が効いた。

> **wasm が 497 KB → 580,700 B に戻っている件**（2026-08-05、`9632474` を worktree に出して同一マシン・同一ツールチェインで測り直した実測）:
>
> |  | `9632474`（`image` 除去直後） | 現在 | 差 |
> |---|---|---|---|
> | wasm | 496,616 B | 580,700 B | +84,084（+16.9%） |
> | JS glue | 93,332 B | 125,867 B | +32,535（+34.9%） |
>
> 間に入ったのは**ロード走査一式（§8.1）・トランスポート・パーツ上書き・パーツ読み出し・イベント**で、いずれも js 面へ露出している。**JS glue が +34.9% 伸びているのがその指紋**——`#[wasm_bindgen]` のメソッドが増えれば wasm 側と JS 側の両方に glue が出る。機能ぶんであって、無駄が混ざったわけではない（release wasm のセクション内訳は code 86.2% / data 9.4%、`name` セクションは wasm-opt が落としている）。
>
> **`ssruntime/std` は原因ではない。** `web` feature の定義は `9632474` と現在で1バイトも変わっておらず、`std` が入ったのは crate 再配置の `43089ea`（`9632474` より前）。つまり 497 KB の時点で既に std 込みだった。

**P7-9（3オブジェクト化）のサイズ代償**（当時、同一マシン・同一ツールチェインで前後を測ったもの。**上の表とは計測時点が違う**——ここは履歴として残す）:

|  | 7-9 前 | 7-9 後 | 差 |
|---|---|---|---|
| wasm | 599,298 B | 624,883 B | +25,585 |
| JS glue | 68,445 B | 83,399 B | +14,954 |
| **gzip 合計** | **253,672 B** | **264,144 B** | **+10,472（+4.1%）** |

エクスポートするクラスが1つから3つになり、メソッド数がおよそ3倍になったぶん。**gzip +10 KB は、canvas を2枚置いた時点で device・パイプライン・アトラスの二重持ちが消えることの対価**として明確に釣り合う（2048² アトラス1枚＝VRAM 16 MB、§1.2）。1要素しか置かない用途でも、払うのは 10 KB だけで損はしない。

> **注意（この訂正の経緯）**: 当初「約10倍・3.8 MB」と書いていたが、それは `build.sh platform=web build=debug`（wasm-pack `--dev`）の成果物だった（wasm-opt 未適用、`name` カスタムセクションだけで 1.76 MB）。**サイズを語るときは必ず release ビルドを測ること。** 計測コマンドは §13。

**方針**: WebGPU 面は「表現力と native 統合」を取りに行く。**到達率**が要件の案件は ssplayer-webgl を使う——サイズは選択理由にならない。`<ss-player-webgpu>` は非対応ブラウザで**エラー表示ではなく WebGL 版へ退避できる形**を目標にする（実装方針は未定）。

### 14.2 「WebGL でできることを wgpu で書いた」箇所の棚卸し（★P8 候補）

P7-6 で頂点生成経路（§5.5）を WebGPU 本来の形へ移したが、**同じ出自の設計は他にも残っている**。WebGL2 の制約——SSBO 無し / テクスチャユニット束縛 / マスクはステンシルのみ / コマンドバッファ再利用なし / シングルスレッドの JS——を前提に書かれた部分で、wgpu では別の答えがある。

以下は棚卸しであって着手順ではない。**まず 10（計測）を入れること**——4〜9 はすべて「速くなるはず」の話で、現状この Player には GPU 側を測る手段が1つも無い。数字が無いまま bindless のような大きな変更に踏み込まない。

#### 表現力（今できないことがある）

| # | 現状（WebGL 由来） | 該当 | WebGPU での本来形 | 備考 |
|---|---|---|---|---|
| 1 | **マスクはステンシル一択**。INCR と INVERT が混ざる重なりマスクは単一ステンシル値で**近似**、8bit なので入れ子の上限も浅い | §6 / `mask.rs` | compute ＋ storage texture の **coverage bitmap**。両方の制約が消える | §6 が「厳密解は coverage-bitmap 方式」と書いたまま止まっている箇所。ライタ数の上限（`MAX_MASK_WRITERS = 24`）の根拠も無くなる |
| 2 | **背景を読むブレンドが Mix フォールバック**。`Div2` / `Overlay2` に「GPU ネイティブ等価が無い」として潰している | §4.3 / `renderer.rs` `blend_state` | 正確には*固定機能ブレンドに*等価が無いだけ。**オフスクリーンへ描いて下地をサンプル**すれば実装できる | group alpha の厳密実装も同じ経路。§14.1 が「安いオフスクリーン＋リードバック」を WebGPU の強みに挙げている当人 |
| 3 | **エフェクトのパーティクル展開が CPU**。runtime がパーティクル1個につき `cosf`/`sinf` を回して4頂点へ展開し、Player はそれを memcpy する | `ssruntime` `lib.rs` の `EffectDrawPlan` 生成 | パーティクル状態（x, y, rotation, scale, color）だけ渡して**頂点ステージで展開**。転送量 1/4、CPU の三角関数ゼロ | **`EffectDrawPlan` の形を変える話なので SDK 側**。このリポジトリでは決められない——SDK への PR ＋ submodule bump（AGENTS.md の「Never commit inside the submodule」） |

#### スループット（できるが WebGL と同じ天井にいる）

| # | 現状（WebGL 由来） | 該当 | WebGPU での本来形 | 備考 |
|---|---|---|---|---|
| 4 | **1 draw = 1 テクスチャ**。GL のテクスチャユニット束縛モデルそのままで、アトラスが違うだけで draw が切れる | `scene.rs` `draw` の `set_bind_group(1, …)` | **bindless**（`binding_array`）で全アトラスを1つの bind group に入れ、`Prim` にテクスチャ index を持たせる | **7-8 の draw_call マージはこれが無いと半分しか効かない**。ただし `Features::TEXTURE_BINDING_ARRAY` は **WebGPU では使えない**——「native 専用の分岐を持つか」という設計判断が先 |
| 5 ❌ **試して却下**（native 計測） | **draw_call 列を毎フレーム re-encode**。GL には代替が無いのでそうなっていた | `scene.rs` `draw` | **render bundle**（記録して再生） | **実装して測ったが全体で 1〜3% 遅くなったので revert**。下記 |
| 6 ❌ **試して撤去**（native 計測） | **プレイヤーを直列に進めていた**。これは WebGL ではなく **JS（シングルスレッド）由来** | `scene.rs` `FrameChunk` / `build_chunks` | Player ごとに `FrameChunk`（arena / prims / mesh_stream / draw_calls / ss_params）を持たせ、独立に組んでから **`order` 順に連結**（`absorb` がオフセットを rebase）。連結順が確定なので結果は分割方法に依存しない | **`FrameChunk` 分解は採用・スレッド化は撤去**して直列 1 本に戻した。下記 |

**6 の計測結果（★「速くなるはず」が外れた例）**

`tests/scale.rs`（14 コア / release / 混成キャスト）:

| players | 直列 | スレッド |
|---|---|---|
| 8 | 0.045 ms | 0.105 ms |
| 32 | 0.164 ms | 0.184 ms |
| 64 | 0.252 ms | 0.241 ms |
| 128 | 0.507 ms | **0.320 ms** |
| 256 | 1.053 ms | **0.580 ms** |

- **64体あたりまでスレッドは損**。`std::thread::scope` の spawn コストが仕事量を上回る——**7-6 で頂点組み立てが CPU から消えた結果、走査そのものが安くなった**ため。
- **そもそも桁を見ること**: 256体でも直列 1 ms。16.6 ms のフレーム予算に対して 6%。**層2 の CPU コストはもう主要因ではない。**
- 当初は `PARALLEL_THRESHOLD = 64` で体数により切り替えていたが、**スレッド化は撤去して直列 1 本に戻した**（`build_chunks`）。理由は3つ:
  - **買えるものが小さい**。効くのは 64体超で、256体でも差は 0.5 ms（予算の 3%）。ゲームが到達しにくい体数のためだけに、コードパスが2本になる。
  - **主配布面の web では使えない**。`wasm32-unknown-unknown` にスレッドが無いので `#[cfg(not(target_arch = "wasm32"))]` の native 専用パスになり、**web ビルドは一度もコンパイルしない**——§14 が「wasm ビルドは必須」と書いている当のリスクを、速度目的のコードで背負うことになる。
  - **永続スレッドプール（rayon）は形が違う**。閾値は下がるが、device すら借用する Player が**ホストのプールと別に自前のスレッドを立てる**のは借用モデル（§1）と相容れない。web では wasm-bindgen-rayon ＋ SharedArrayBuffer ＋ COOP/COEP まで要求する。
- **撤去時の実測（同一マシンで before/after、`frame_cost_by_player_count`。`update` ＋ pass encode の CPU 合計）**:

  | players | スレッド（撤去前） | 直列（撤去後） | 差 |
  |---|---|---|---|
  | 8 | 0.385 ms | 0.379 ms | ±0 |
  | 32 | 0.765 ms | 0.801 ms | +0.036 |
  | 64 | 1.049 ms | 1.148 ms | +0.099 |
  | 128 | 1.522 ms | 1.632 ms | +0.110 |
  | 256 | 1.808 ms | 2.339 ms | **+0.531** |

  GPU 時間は 0.185 ms で不変（層2 の CPU 走査の話なので当然）。**払ったのは 256体で 0.53 ms**、それでも 16.6 ms 予算の 14%。上の 14 コア機の表と絶対値が違うのは測定機が別だから——**比較は必ず同一マシンの before/after で**取ること。
- **そもそも「体数で殴る」ケースは別機能で受ける**: 同じ `.ssab` の同じアニメを大量に出す用途は、**N 体の Player を進める**のではなく **1データ : N インスタンス（群衆）** で描く。走査も `FrameData` も1体分のまま `draw(vertices, 0..N)` になるので、**CPU は体数に比例しない**——スレッド化が縮めようとしていたコストは、そもそも発生しない。§14.3 を参照。
- **`FrameChunk` への分解自体は残す**: Player 間の結合が切れ、直列時のコストはゼロ（`absorb` の rebase のみ）で、**将来ここが重くなったら並列版はこのループを `scope` で囲むだけ**に戻せる。再測定は `cargo test --release --test scale -- --ignored --nocapture`——数字が桁で動かない限り再挑戦しないこと。

**★この却下は「毎フレームの Player ウォーク」に限る**（`decode.rs`、✅ 実装済）

上の3つの理由は**予算の数%を毎フレーム削る話**に対するもので、**ロード時のアトラスデコードには当たらない**。区別して読むこと:

- **買えるものが小さい、が当たらない**。デコードは1枚 4096² で 29〜51 ms、3枚で 86 ms。16.6 ms 予算の数%ではなく**予算を5フレームぶん飛ばす一発のストール**で、ユーザーには「D&D したら固まる」として見える。
- **web で使えない、が当たらない**。デコード経路は `embedded` feature 配下にあり、`web` は意図的に `embedded` を含めない（§10：ブラウザが既にデコーダを持っているため）。**web ビルドは元からこのコードをコンパイルしていない**ので、「native 専用パスを web が一度もコンパイルしない」というリスクは新たに増えない。
- **永続プールが借用モデルと相容れない、は同意**。だから常駐プールも rayon も入れない。`std::thread::scope` で**呼び出しから戻る前に消える**短命スレッドだけを使い、依存は増やさない（`decode.rs` `decode_all`）。
- **順序は守る**。完了順は非決定的なので、重複排除を「デコード後に `texture_size` を見る」から**ディスパッチ時に確定させる**方へ前倒しした（`player.rs` `flush_textures`）。reference が embedded に勝ち、embedded 同士はパック名昇順、という §8.1 の「先に登録したものが勝つ」は完了順に依存しない。

**計測（同一マシン・release、4096²×2 ＋ 2048²×1 = 37.6 Mpx の reference モードパックを viewer に D&D、ドロップ直後の1フレーム）**:

| | before | after |
|---|---|---|
| ドロップ直後のフレーム | 134 ms | **74 ms** |
| 内訳: PNG デコード | 86 ms（直列） | ワーカーへ |
| 内訳: premul ループ | 17 ms | ワーカーへ |
| 内訳: 全画素コピー2本 | 13 ms | **0** |
| 内訳: `create_texture` ＋ `write_texture` | 12 ms | 12 ms（ホスト側に残る） |
| ピーク CPU ピクセル | 450 MB | **150 MB** |

`frame_cost_by_player_count` は before/after で不変（256体 2.03 ms）——定常フレームには触っていないので当然。残りの 74 ms は**一番重い1枚のデコード（51 ms）＋ GPU 書き込み**で、これ以上縮めるにはフレームをまたぐ非同期化が要る。それは「アトラスは load の次の `update` で上がる」（§8.1）という契約を変えるので**別の判断**。
| 7 | **テクスチャ bind group を毎 draw 無条件に再設定**。同じテクスチャが続いても再バインドする | `scene.rs` `draw` | 直前と同じならスキップ（pipeline 側の `_lastBlend` 相当の去重がテクスチャには無い） | 小さいが 4 の前段として自然 |
| 8 | **Shape は常に 30 頂点**（`SHAPE_FAN_VERTS` 固定ストライド）。余りは面積ゼロに潰している | §5.5 / `renderer.rs` `vs_shape` | prefix sum ＋ **indirect draw** で実頂点数ぶんだけ回す | Shape は稀なので優先度は低い。7-6 で「算術で引ける」ことを優先した意図的な割り切り |
| 9 ✅ **native 側は解消**（web は未着手） | **テクスチャの premultiply を CPU でやる**。WebGL は `UNPACK_PREMULTIPLY_ALPHA_WEBGL` がドライバ側で無料だった | §8 / `renderer.rs` `upload_texture` | web は `createImageBitmap` ＋ `copy_external_image_to_texture` で**デコードをオフスレッド・premul を GPU へ** | native は**一時コピー0本＋premul をデコードワーカーへ**で片付いた（§8 / §8.1）。premul そのものは残す（線形補間の正しさのため、§14）。web 側の案は据え置き |

**5 の計測結果（★実装して却下した例）**

draw リストが前フレームと同一なら bundle を再生、変われば録り直す実装で計測（`tests/scale.rs`、`update` ＋ pass encode を**同一ループで**）:

| players | 直接 encode | render bundle | 録り直し率 |
|---|---|---|---|
| 8 | 0.0578 ms | 0.0600 ms | 115/200 |
| 64 | 0.2671 ms | 0.2742 ms | 115/200 |
| 256 | 0.6633 ms | 0.6822 ms | 115/200 |

- **draw リストは 58% のフレームで変わる**。パーツの表示/非表示で prim 数が動き、vertex range がずれるため。録り直すフレームは「直接 encode ＋ リスト比較 ＋ bundle finish」で確実に損。
- 再生できる 42% で浮くのは encode ぶんだけで、**encode は 256体でも 0.09 ms＝フレームの 13%**。差引きマイナス。
- 加えて **bundle は pass の depth/stencil 読み書き属性と一致する必要**があり、ホストの pass 記述子に新しい制約が増える。得るもののほうが小さい。
- **計測の落とし穴**: `update` と encode を別ループで測ると bundle は 0.0898→0.0003 ms（300倍）に見える。更新しないループでは録り直しが起きないため。**キャッシュ系を測るときは必ず実フレーム順で回すこと。**
- **成立させる道が1つだけある**: `draw_indirect` で vertex range をバッファへ追い出せば、リストは安定し録り直しがほぼ消える。ただし上限は依然 0.09 ms なので、現状では割に合わない。
- **web は未計測**。WebGPU では1コマンドが1 JS 呼び出しなので比率が変わる可能性はある。**ブラウザで測ってから**再検討すること——計測器は P7-9 で入った（`surface.stats()`、§10）。

**7 / 4 の当たり取り（★GPU 側の実測）**

`tests/scale.rs`（1280x720、混成キャスト、release）。`mergeable` は「直前の draw と pipeline・texture が同じで vertex range が連続する」数＝**マージの上限**、`rebinds` は「直前と同じテクスチャなのに bind group を張り直している」数＝**7 の上限**。

| players | CPU (ms) | **GPU (ms)** | prims | draws | mergeable | rebinds |
|---|---|---|---|---|---|---|
| 8 | 0.26 | 0.043 | 234 | 78 | 24 (31%) | 65 (83%) |
| 64 | 0.64 | 0.136 | 1872 | 624 | 192 (31%) | 527 (84%) |
| 256 | 1.33 | **0.185** | 7488 | 2496 | 768 (31%) | 2111 (85%) |

- **draw の 85% は bind group の張り直しが不要**（同一アトラスを共有するため）。7 の効果は確かにあるが、**削れるのは CPU の encode 側だけ**で、encode は 256体でも 0.09 ms。
- **マージできるのは 31%**。2496 → 1728 draws。
- **しかし GPU は 256体でも 0.185 ms**。16.6 ms の予算に対して 1.1%。draws を 31% 削っても、削れるのはその一部でしかない。
- **結論: 7 も 4 も、現状のワークロードでは投資に見合わない。** 4（bindless）は WebGPU で使えず native 分岐が要る話でもあり、**0.1 ms 未満を買うために持ち込む複雑さではない**。
- **ただし `rebinds` 85% は web では話が変わりうる**（1コマンド＝1 JS 呼び出し）。ブラウザで測ってからにすること。7 は実装も小さいので、web の数字が出たら再考する価値はある。**取り方は `surface.stats()`**（P7-9、§10）——`examples/web` の②が毎フレーム表示する。

#### 計測

| # | 現状（WebGL 由来） | WebGPU での本来形 | 備考 |
|---|---|---|---|
| 10 ✅ **実装済** | **GPU 側を測る手段が無かった**。WebGL の `EXT_disjoint_timer_query` は Spectre 対策でブラウザから実質消滅したので、参照実装には読む時計自体が無く、読む習慣も育たなかった | `metrics.rs`：`SceneStats`（players / prims / draw_calls / uploaded_bytes）＋ **timestamp query** による pass 単位の GPU 時間 | 下記参照 |

**10 の実装メモ**

- `SSScene::stats() -> SceneStats`。**カウンタは常に取れる**（無料）。`prims` と `draw_calls` の差が、そのまま 4（bindless）と 7-8（draw マージ）の伸びしろ。
- GPU 時間は `Features::TIMESTAMP_QUERY` が要る。**`request_device` 時に要求しないと一生使えない**ので、各ホストが `adapter.features() & TIMESTAMP_QUERY` で「あれば取る」形にしてある（`host.rs` / `web.rs` / `examples/viewer`）。無い環境でも動作は変わらず、数字だけ `None` になる。
- **pass 単位**（`timestamp_writes` の beginning/end）に留めた。draw 単位は `TIMESTAMP_QUERY_INSIDE_PASSES` が要り、これは native 限定。「その変更でフレームは安くなったか」には pass 単位で答えられる。
- 借用モデルの帰結として、**render pass を開くのはホスト**なので、scene は `timestamp_writes()` を渡すだけ。ホストは pass 記述子に入れ、pass 後に `resolve_timestamps(&mut encoder)` を呼ぶ。
- **読み出しは1〜2フレーム遅れる**。パイプラインを止めずに読むとはそういうこと。プロファイル用カウンタとしては正しい割り切りで、`begin_frame` が「前フレームの submit 済み」を利用して map を仕掛ける。
- **CPU 時間は意図的に持たない**。`std::time::Instant` は `wasm32-unknown-unknown` で panic するため、ここで測ると依存を1つ増やして、ホストが1行で書けるものを肩代わりすることになる（viewer は `Instant` で `update` を挟んでいる）。
- C 面は `ss_scene_stats(ss_scene_t*, ss_scene_stats_t*)`。Metal viewer が表示する。
- 検証: `tests/scene.rs::a_frame_reports_what_it_cost`（カウンタが実際に増減するか）＋ `gpu_pass_time_comes_back`（**timestamp のハンドシェイク全体**を歩いて数字が返ることを要求。timestamp query 非対応アダプタでは skip）。実測 0.025 ms / 256² / Basic.ssab 1体。

**★結論（計測ののち）**: **§14.2 の速度側 6 項目（4〜9）は、現状のワークロードでは着手する理由が無い。**

- 10（計測）✅ → 6（並列化）❌ 64体未満で損・64体超の 0.5 ms のために native 専用パスは持たない、撤去 → 5（render bundle）❌ 全域で損、revert → **7 / 4 は当たりを取った結果、上限が小さすぎて見送り**。**速度側で残ったのは `FrameChunk` 分解だけ**（項目6 の副産物、直列でもコスト0）。
- 数字がすべてを説明する: **256体で CPU 1.3〜2.3 ms ＋ GPU 0.19 ms**（直列に戻した後の実測レンジ。測定機で振れる）。60fps の予算 16.6 ms に対して合計 15% 以下。**P7-6 が CPU 側の仕事を実際に消した**ため、その先に積む最適化は削る対象を失っている。
- 8（Shape 固定30頂点）・9（CPU premul）も同じ理由で優先度は低い。9 は web でのアップロード一時コピー 16 MB の話なので、**サイズ/メモリの観点**なら別途価値がある。
- **残る価値は速度ではなく表現力**——1（マスクの厳密化）と 2（背景読みブレンド）は「今できないことができるようになる」項目で、計測とは無関係に効く。**次に着手するならこちら。**
- **web だけは未計測**。1コマンド＝1 JS 呼び出しなので 5 / 7 の比率は変わりうる。**`surface.stats()` は P7-9 で JS へ出したので、あとはブラウザで読むだけ**（`examples/web/index.html` の②が表示する）。1 / 2 は表現力なので必要になってから。3 は SDK 側の合意が要る。
- **7-8（draw マージ・マスクのランレングス結合）もここに合流して保留**。`mergeable_draws` が数えている 31% が上限で、削れるのは encode 0.09 ms のその 31%。項目 7 / 4 と同じ結論になる（§12.1）。

### 14.3 群衆（1データ : N インスタンス）★未実装・設計中

同じ `.ssab`・同じアニメ・同じ再生位置の体を大量に出す用途を、**Player を N 個持たずに**描く。§14.2 項目6（走査のスレッド化の撤去）と対になる決定——**体数の問題は「走査を速くする」ではなく「走査を1回に減らす」で解く。**

**この Player の頂点経路と噛み合う**（だから安い）:

- 頂点ステージは4クラスとも最後に `apply_node(node_base, …)` を通り、`node_base` が指すのは `begin_node` がアリーナへ積んだ**プレイヤーの配置行列 1枚**（§5.5）。
- よって群衆は「配置行列を N 枚連続で積み、`@builtin(instance_index)` で選ぶ」だけ——`apply_node(base + inst * 16u, …)` と `rpass.draw(vertices, 0..N)` の2点。
- **非群衆は `inst = 0` ＝ オフセット 0** なので演算列は1命令も変わらない。`golden_corpus` のビット一致がそのまま維持される（§5.5 の FMA 契約を壊さない）。
- CPU 増分はアリーナへ 64 B × N（256体で 16 KB/frame）。`update` と `FrameData` は1体分のまま＝**CPU は体数に比例しない**。

**原理的に持てないもの（採用条件）**:

| 制約 | 理由 | 回避 |
|---|---|---|
| **メンバーごとの再生位置** | `FrameData` の共有が本体 | 位相を散らすなら「位相 K 種＝群衆 K 個」に分ける |
| **重なったメンバーの前後関係** | 1 draw ＝ そのパーツを全メンバーぶん。**パーツ単位で層になる**ので、A の頭が B の足より先に出る | 重ならない配置（群衆の本来の用途）に限る。重なるなら通常の Player |
| **マスク（ステンシル）** | ライタも instanced になり、N 人ぶんの和集合がステンシルに乗る | 群衆はマスク持ちパックを受け付けない |
| **エフェクト** | `EffectDrawPlan` 共有＝全員同一のパーティクル | 同上、または同一を許容する |

**未決（着手時期も未定——ここは現時点ではメモ）**: API の形（`SSPlayer::set_instances(&[Transform])` か新ハンドル `SSCrowd` か）、メンバー単位で変えられる属性（transform だけ／＋alpha／＋tint）、js・C 面の露出。**決めてから書く。**

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

**現行 wgpu 実装（P7 完了後）**
- `libs/ssplayer-wgpu/src/renderer.rs`（層2a・device スコープ）／ `scene.rs`（層2b・ターゲットスコープ）
- `libs/ssplayer-wgpu/src/player.rs`（再生木。GPU リソースを一切持たない）／ `pack.rs`（`.ssab` の共有単位）
- `libs/ssplayer-wgpu/src/web.rs`・`host.rs`・`ffi.rs`（各ホスト。いずれも 1 ターゲット N 体）／ `roster.rs`（ハンドル）
- `examples/viewer/src/main.rs`（winit+wgpu ループ。1 パスに N 体、体ごとにパック/アニメを選べる目視ツール——§13 の「複数プレイヤー（目視）」）

**全体方針**
- `REDESIGN_ASSESSMENT.ja.md`（頭脳/体、オーバーライド/ミキシング/ステートマシンの将来計画）
