# SSPlayer for wgpu ドキュメント

**SSPlayer for wgpu** は、SpriteStudio 7 アニメーションを **WebGPU / Vulkan / DirectX 12 / Metal / OpenGL** で高速描画するための公式レンダラー・プレイヤーライブラリです。

---

## 🎯 特徴

1. **モダン GPU ネイティブ (wgpu)**
   - 単一の Rust / WGSL 実装で WebGPU, Vulkan, DirectX 12, Metal, OpenGL のすべてを網羅。
   - Programmable Vertex Pulling（頂点バッファレス描画）による圧倒的な低 CPU 負荷。
2. **借用モデル (Borrow Model)**
   - プレイヤーは GPU デバイスやコンテキストを専有・所有せず、ホストから借りて描画。
   - 任意のゲームエンジン（Bevy / Slint / egui / 自作エンジン / ネイティブ C++ アプリ）へロックインなしで相乗り可能。
3. **1 シーンに複数プレイヤー (N 体まとめ描画)**
   - 1 つの描画ターゲットに任意の数のプレイヤーを配置し、1 つのレンダーパスで一括描画。
4. **3 面提供（Rust / C++ / Web）**
   - **Rust API**: 第一級市民としてのクリーンなイディオマティック Rust API。
   - **C / C++ API (`ffi`)**: DirectX 12 / Metal などの C++ ホストから呼び出せるヘッダ付き C-API。
   - **WebGPU / JS (`web`)**: WebAssembly による `<ss-player-webgpu>` Web Component および JavaScript API。

---

## 📚 ドキュメント目次

### 導入・マイグレーション
- 🔄 **[SS6Player for CPP からのマイグレーション](./migration_from_cpp.md)** — 旧 C++ プレイヤーからの移行ガイド・API 対応表・実践コード比較

### ライブラリ API
- 📦 **[クレート README (`libs/ssplayer-wgpu`)](../../libs/ssplayer-wgpu/README.md)** — 借用モデル、3 つの型、feature、使い方

### 開発・設計仕様
- 🗺️ **[ROADMAP](../../ROADMAP.md)** — 実装済み・未実装・意図的に採用しなかったもの
- 🤖 **[AI エージェント / 開発ガイド (AGENTS.md)](../../AGENTS.md)** — 不変条件、ビルド手順、テスト方法、検証ルール

### サンプル
- 🖥️ **[Native Viewer](../../examples/viewer/)** — winit ベースの Rust ネイティブビューア
- 🌐 **[WebGPU Demo](../../examples/web/README.md)** — ブラウザ上で動作する WebGPU デモ
- 🍏 **[Metal C++ Viewer](../../examples/viewer-metal-cpp/README.md)** — macOS / Cocoa (Objective-C++) ホストサンプル
- 🪟 **[DX12 C++ Viewer](../../examples/viewer-dx12-cpp/README.md)** — Windows / Win32 (C++) ホストサンプル
- 🐧 **[Vulkan C++ Viewer](../../examples/viewer-vulkan-cpp/README.md)** — Linux / X11 (Xlib) と Wayland を 1 バイナリに収めた C++ ホストサンプル（未実行）
