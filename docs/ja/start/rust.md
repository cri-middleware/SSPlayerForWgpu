---
title: Rust ではじめる
---

# Rust ではじめる

**このページは `wgpu::Device` を自分で持っている Rust アプリケーション向けです** — 自作レンダラー、
`winit` のウィンドウ、`egui`、`Slint` など、wgpu の上に載っているものなら何でも。Player は *あなたが*開いたレンダーパスの中に描画します。デバイスもキューもサーフェスも、自分では作りません。

**Bevy アプリ**の場合は [Bevy のパス](./bevy.md)へ。アダプタがこのページの内容を代わりにやってくれます。

## 前提 { #requirements }

| | |
|---|---|
| Rust | 1.99 以降、edition 2024 |
| wgpu | **30** 系 — アプリと Player は 1 つの `wgpu` に解決される必要があるため、メジャーが違うとコンパイルエラーになります |

## 導入 { #install }

クレートを git 依存として追加します。`SpriteStudio-SDK` サブモジュールは cargo が取得します：

```toml
[dependencies]
ssplayer-wgpu = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }
```

**自分のツリーにソースを取り込む**場合（自リポジトリのサブモジュールにする、手元で patch するなど）は、リポジトリのルートではなく**クレートのディレクトリ**を指します：

```toml
[dependencies]
ssplayer-wgpu = { path = "third_party/SSPlayerForWgpu/libs/ssplayer-wgpu" }
```

このパスは workspace ではなく 1 クレートなので、あなたの workspace にそのまま収まります。チェックアウト先で
`git submodule update --init --recursive` を忘れずに — Player 自身の `SpriteStudio-SDK` サブモジュールが
path 依存になっており、これが無いと `ssruntime` / `ssab` を解決できません。

## 3 つのオブジェクト

| | スコープ | いくつ持つか |
|---|---|---|
| `SSRenderer` | デバイス | `wgpu::Device` ごとに 1 つ。**ウィンドウやキャンバスごとではありません** |
| `SSScene` | 描画ターゲット | 描き込む先（サーフェスやテクスチャ）ごとに 1 つ |
| `SSPlayer` | キャラクター | 画面上のキャラクター 1 体ごとに 1 つ。GPU リソースを持たないので、1 シーンに何体でも載ります |

詳しくは[スコープと名前の対応](../guide/concepts.md)、設計理由は[借用モデル](../wgpu/borrow-model.md)。

## 初期化（1 回）

```rust
use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};

// デバイススコープ。パイプライン・サンプラー・アトラステクスチャ。
let mut renderer = SSRenderer::new(&device);

// ターゲットスコープ。`color_format` は描画先と一致している必要があります。
let mut scene = SSScene::new(&device, &SceneConfig {
    color_format: surface_format,
    stencil_format: Some(wgpu::TextureFormat::Depth24PlusStencil8),
    mask: MaskBackend::Stencil,
});
scene.set_viewport(&queue, width as f32, height as f32, /* y_down */ false);

// キャラクター 1 体。
let mut player = SSPlayer::new();
player.load("assets/overall/Basic.ssab")?;   // パック・アトラス・兄弟パックを 1 呼び出しで
player.set_animation("anime_1")?;            // 選択と再生開始は別
player.play();
```

`load` は GPU に一切触りません。借用モデルではフレームが始まるまで何も貸し出されないため、読み込まれたデータは最初の `update` で GPU へ渡ります。[ロードとアセット](../guide/loading.md)を参照。

## 毎フレーム

```rust
// 1. 全プレイヤーを進めて、このターゲットのジオメトリを構築する。
scene.update(&mut renderer, &device, &queue, delta_secs, &mut [&mut player]);

// 2. シーンがオフスクリーンに描くもの — カバレッジマスクのビットマップと
//    group alpha のレイヤー — を描く。どちらも無ければ何もしない。
//    下のパスと「同じ encoder」に積む必要があります。
scene.encode_offscreen(&mut renderer, &device, &mut encoder);

// 3. 自分で開いたパスに描画を記録する。
let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
    label: Some("ss pass"),
    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
        view: &frame_view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
    })],
    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
        view: &stencil_view,
        depth_ops: None,
        stencil_ops: Some(wgpu::Operations {
            load: wgpu::LoadOp::Clear(0),
            store: wgpu::StoreOp::Store,
        }),
    }),
    ..Default::default()
});
let draw_calls = scene.draw(&renderer, &mut rpass);
```

そのターゲットの**全キャラクターを 1 回の `update` に渡し、1 パスで描画してください**。10 体でもアトラスアップロード 1 回・パイプライン 1 セットで済むのはそのためです。前後関係は `SSPlayer::set_order` の昇順です。

パスが守るべきことは 4 つ：

* **カラーフォーマットが `SceneConfig::color_format` と一致していること。** パイプラインはこれに対して焼かれているので、フォーマットが変わるなら `SSScene` を作り直します。
* **`MaskBackend::Stencil` ならステンシルアタッチメントが必須**（宣言したフォーマットで）。貸せるステンシルが無い場合は `MaskBackend::Coverage` を使ってください（[マスク](../guide/masks.md)）。
* **MSAA はオフ。** Player のパイプラインはシングルサンプルです。
* **非 sRGB ターゲットを渡すこと。** Player はアトラスのバイト列をそのまま書き、色管理をしません。
  sRGB ターゲットでは二重にエンコードされて全体が明るくなりすぎます。`examples/hello` は
  `capabilities.formats.iter().find(|f| !f.is_srgb())` でサーフェスフォーマットを選んでいます。
  sRGB 空間で合成する方法でも構いません。

リサイズ時は自分のアタッチメントを作り直して `set_viewport` を呼び直します。

## サンプルを動かす

```bash
git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
cd SSPlayerForWgpu
./scripts/deploy-examples.sh          # SDK のテストプロジェクトを assets/ に変換
cargo run -p ssplayer-wgpu-hello
```

[`examples/hello`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/examples/hello/src/main.rs) は、このページの手順をそのまま 1 つのプログラムにしたものです。winit のウィンドウにキャラクターを 1 体、ウィンドウに収めて表示します。ほかの機能 — パック切り替え、描画順、マスクバックエンド、パーツオーバーライド、モーションブレンディング、統計表示 — を触るときは `cargo run -p ssplayer-wgpu-viewer` を使ってください。

## 次に読む

* [再生と再生制御](../guide/playback.md) — `play` が実際にやること、`resume` がある理由
* [ロードとアセット](../guide/loading.md) — パスの解決方法と、エンジンに載せる場合のやり方
* [借用モデル](../wgpu/borrow-model.md) — 設計理由と、既存のレンダーグラフへの組み込み方
