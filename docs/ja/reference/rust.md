---
title: Rust API リファレンス
---

# Rust API リファレンス

生成されたリファレンスはこのドキュメントと一緒に公開されています：

<a href="../../api/ssplayer_wgpu/index.html" target="_blank" rel="noreferrer">**📦 `ssplayer_wgpu` の rustdoc**</a>
&nbsp;·&nbsp;
<a href="../../api/ssplayer_bevy/index.html" target="_blank" rel="noreferrer">**`ssplayer_bevy` の rustdoc**</a>

手元では `cargo doc --no-deps --all-features -p ssplayer-wgpu --open` で同じものが見られます。

このページはその見取り図です。何がどこにあり、どの feature で有効になるか。

## 公開されているもの

```rust
pub use events::{AudioEvent, SignalEvent, SignalParam, SignalValue, SsEvent, UserDataEvent};
pub use fit::{Align, Fit, Placed, Placement, Rect};
pub use frame::FrameView;
pub use mask::{GeomKind, MaskOp, MaskWriter, MaskWriters, PartMaskFlags, ScopedWriter, StencilTest, plan_writers};
pub use metrics::SceneStats;
pub use pack::{EmbeddedImage, EmbeddedTexture, SSPack, TextureRef};
pub use player::{AssetLoader, AtlasKeys, BlendOp, MissingAsset, MissingKind, OverridePriority,
                 PlaybackDirection, PlaybackStyle, SSPlayer};
pub use renderer::{AlphaMode, Retention, SSRenderer};
pub use scene::{BatchInfo, FrameChunk, IDENTITY_TRANSFORM, MaskBackend, NodeSlot, SSScene, SceneConfig};
pub use ssruntime::TrackBlend;
pub use ssruntime::TrackSync;
pub use ssruntime::error::SSRuntimeError;
#[cfg(feature = "web")]
pub use web::{SsContextWeb, SsPlayerWeb, SsSurfaceWeb};
pub const VERSION: &str;      // この Player のバージョン
pub const SDK_VERSION: &str;  // リンクしている SpriteStudio-SDK のリリース。リリースでなければ "local"
```

ほとんどのアプリが触るのはこのうち 6 つです：`SSRenderer` / `SSScene` / `SceneConfig` / `SSPlayer` /
`MaskBackend` / `SsEvent`。

## 3 つの型

| 型 | スコープ | 主な呼び出し |
|---|---|---|
| `SSRenderer` | デバイス | `new`, `set_texture`, `upload_texture`, `texture_size`, `restore_device` |
| `SSScene` | 描画ターゲット | `new`, `set_viewport`, `update`, `update_with`, `encode_mask_coverage`, `draw`, `stats`, `restore_device` |
| `SSPlayer` | キャラクター | `load`, `set_animation`, 再生制御, オーバーライド, `set_cellmap_texture`, `events`, `set_transform`, `restore_textures` |

`SSPlayer` は **wgpu の型のフィールドを一切持ちません**。だから 1 つのシーンに何体でも載り、パックのアトラスのアップロードを共有します。[借用モデル](../wgpu/borrow-model.md)を参照。

## モジュール

| モジュール | 役割 |
|---|---|
| `renderer` | デバイススコープ：パイプライン（初回使用時に構築）、シェーダー、テクスチャマップ |
| `scene` | ターゲットスコープ：投影、フレームアリーナ、ドローリスト、GPU 計測 |
| `player` | 再生ノードツリー、ロード、再生制御、パーツオーバーライド |
| `pack` | `SSPack` — キャラクター間で共有される `.ssab`。CPU データのみ |
| `fit` | オーサリング時のキャンバスを箱に合わせる：`Fit`、`Align`、`Placement` |
| `assets` / `decode` | ローダーの継ぎ目と、アトラスの一括デコード |
| `mask` | ステンシルプランナ |
| `events` / `metrics` / `roster` | タイムラインイベント、フレームカウンタ、世代付きハンドル |
| `host` | ウィンドウを持つ wgpu スタック（`ffi` feature）。自前で wgpu を組まず生のウィンドウハンドルを持つ Rust ホストからも使えます |
| `ffi` / `web` | C と JavaScript の Face |

## feature

| feature | 既定 | 追加されるもの |
|---|---|---|
| `embedded` | ✅ | 埋め込みモードのアトラスをクレート内でデコードし、`SSPlayer::load` のファイルパス経路を提供します。ホスト側でデコードする場合はオフに |
| `web` | | wasm-bindgen 経由の `SsContextWeb` / `SsSurfaceWeb` / `SsPlayerWeb`。**意図的に `embedded` を引き込みません** — ブラウザが既にデコーダを持っているためです |
| `ffi` | | C のエントリポイントと、生のウィンドウハンドルから wgpu を構築する `NativeHost` |

クレートはただの `rlib` です。姉妹クレート `ssplayer-wgpu-ffi` がこのクレートの `ffi` / `web` モジュールを再エクスポートし、`cdylib` ＋ `staticlib`（ネイティブ C ライブラリ）と wasm モジュールとしてビルドされる側です。つまり 1 つのソースが Rust の依存・ブラウザ・ネイティブ共有ライブラリの 3 役を担います。

!!! note "`SSPlayer::load` には `embedded` とファイルシステムが必要です"
    この簡便な経路は `#[cfg(all(feature = "embedded", not(target_arch = "wasm32")))]` です。それ以外（wasm、アーカイブ、エンジンのアセットサーバー）では `load_with(&mut loader, src)` が入口になります。[ロードとアセット](../guide/loading.md)を参照。

## エラー

ロードは `Result<(), SSRuntimeError>` を返し、**`.ssab` 本体が読めなかったときだけ**失敗します。それ以外に届かなかったものは `missing_assets()` にあります。

オーバーライドやトラック系の多くも `Result<(), SSRuntimeError>` を返し、何もロードされていないか名前が解決できない場合に `Err` になります。

## バージョニング

`ssplayer-wgpu` と `ssplayer-bevy` は**独立**にバージョン管理されています。追いかける対象が違うからです（Player は SDK、アダプタは Bevy のリリース系列）。

wgpu はリポジトリ全体で **30** 系に固定されています。[エンジンへの組み込み](../wgpu/engines.md#the-wgpu-pin)を参照。
