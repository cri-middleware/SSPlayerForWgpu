# C / C++ ではじめる

静的ライブラリ 1 本とヘッダ 1 枚。ウィンドウ（あるいはレイヤー、サーフェス）を渡して毎フレーム
2 つの関数を呼ぶと、**Windows では DirectX 12、macOS では Metal、Linux では Vulkan** で描画されます。

!!! tip "wgpu を知っている必要はありません"
    wgpu は「1 つのライブラリで 3 つのグラフィックス API を賄う」ための手段であり、ここでの関わりは
    それだけです。**wgpu の型はこの ABI を一切越えず**、ヘッダにも登場しません。渡すのはプラットフォームの
    ウィンドウハンドルで、返ってくるのは `SS_OK` かエラーコードです。

    1 点だけ先に知っておくべきこと：この Face は**自分でグラフィックスデバイスを持ちます**。あなたの
    `ID3D12CommandList` や `MTLCommandBuffer` に描き込むわけではありません。エンジンのデバイスを C ABI 経由で
    共有することは[意図的に対象外](../limitations.md#no-device-sharing-across-the-c-abi)です。C から合成したい
    場合の経路はオフスクリーン描画＋テクスチャ共有になります。

## 手に入るもの

| | |
|---|---|
| `include/ssplayer_wgpu.h` | ヘッダ 1 枚。C99、依存なし。cbindgen が生成 |
| `lib/libssplayer_wgpu.a`（MSVC では `ssplayer_wgpu.lib`） | 静的ライブラリ |
| `LICENSE.md` | BSD-3-Clause |

## 導入 { #install }

=== "リリース版"

    🚧 **まだリリースされていません。** これらをビルドするワークフロー
    （`.github/workflows/release.yml`）は存在し、最初のタグで公開されます。公開後は
    [Releases ページ](https://github.com/cri-middleware/SSPlayerForWgpu/releases)にプラットフォームごとの
    zip（`include/` `lib/` `LICENSE.md`）が並びます：

    | zip | 対象 |
    |---|---|
    | `ssplayer-wgpu-windows-x86_64.zip` | Windows / MSVC（DX12） |
    | `ssplayer-wgpu-macos-universal.zip` | macOS。arm64 + x86_64 が 1 本に（Metal） |
    | `ssplayer-wgpu-linux-x86_64.zip` | Linux x86_64（Vulkan） |
    | `ssplayer-wgpu-linux-aarch64.zip` | Linux aarch64（Vulkan） |

    **意図的に静的ライブラリのみです。** C++ ホストはライブラリを自分のバイナリにリンクします。これは
    この Face が自分でデバイスを持つことの帰結でもあります。`.dll` / `.dylib` / `.so` も同梱すると、
    同じことをする 2 つ目の方法のために、コード署名・SONAME・インポートライブラリの対応付けを毎回の
    リリースに背負うことになります。

=== "ソースから"

    必要なのは Rust ツールチェイン（1.97+）だけです。同じビルドで cbindgen がヘッダも再生成するため、
    ヘッダは常に今ビルドしたライブラリと一致します。

    ```bash
    git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
    cd SSPlayerForWgpu
    cargo build --release -p ssplayer-wgpu-ffi
    ```

    | 場所 | 内容 |
    |---|---|
    | `target/release/` | `libssplayer_wgpu.a` **と** `libssplayer_wgpu.dylib` / `.so`。MSVC では `ssplayer_wgpu.lib` / `ssplayer_wgpu.dll` / インポートライブラリ `ssplayer_wgpu.dll.lib` |
    | `libs/ssplayer-wgpu-ffi/include/ssplayer_wgpu.h` | このビルドで再生成されたヘッダ |

    `./scripts/build.sh`（Windows では `build.ps1`）も同じことをしつつ、結果を `bin/` に収集します
    （`bin/include/`、`bin/libs/<platform>/<arch>/`）。これはリリース zip の元になるレイアウトです。
    macOS で 2 スライスのライブラリが欲しい場合は `arch=universal` を付けます。

## リンク

ライブラリは Rust 製なので、**静的**リンクにはツールチェインが本来指定してくれるプラットフォーム
ライブラリが必要です。対象プラットフォーム上でツールチェインに聞いてください：

```bash
cargo rustc -p ssplayer-wgpu-ffi --release --crate-type staticlib -- --print native-static-libs
```

macOS ではこう返ります：`-lSystem -liconv -framework QuartzCore -framework Metal -framework
Foundation -framework CoreFoundation -lobjc`。これに自分のウィンドウコードが必要とするもの
（`-framework Cocoa`）を足します。CMake なら：

```cmake
add_executable(myapp src/main.cpp)
target_include_directories(myapp PRIVATE "${SSPLAYER_DIR}/include")
target_link_libraries(myapp PRIVATE
    "${SSPLAYER_DIR}/lib/libssplayer_wgpu.a"
    "-framework Cocoa" "-framework QuartzCore" "-framework Metal"
    "-framework Foundation" "-framework CoreFoundation" objc iconv)
```

リポジトリ内の 3 つのサンプルビューアは「ソースから」の経路を取り、動的ライブラリをリンクしているため
上記は不要です。その形は
[`examples/viewer-metal-cpp/CMakeLists.txt`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/examples/viewer-metal-cpp/CMakeLists.txt)
を参照してください。

## ウィンドウの指定

`ss_window_handle_t` はプラットフォームのウィンドウを ABI 用に平坦化したものです。`kind` が使うフィールドだけ
埋め、残りは null にします。

| `kind` | `window` | `display` |
|---|---|---|
| `SS_WINDOW_WIN32` | `HWND` | `HINSTANCE`（null 可） |
| `SS_WINDOW_APPKIT` | `NSView *` | — |
| `SS_WINDOW_METAL_LAYER` | `CAMetalLayer *` | — |
| `SS_WINDOW_XLIB` | `Window` XID をポインタにキャストしたもの | `Display *`（＋ `screen`） |
| `SS_WINDOW_WAYLAND` | `wl_surface *` | `wl_display *` |

ウィンドウ（macOS ではそのレイヤーも）はシーンより長生きしている必要があります。

!!! warning "Wayland 上でも X11 ウィンドウは X11 のまま"
    Xwayland があるため、X11 ウィンドウを開いたツールキットはデスクトップセッションが何であれ
    `SS_WINDOW_XLIB` のままです。コンポジタではなく、**自分が実際に作ったもの**で `kind` を選んでください。

## プログラム全体

```c
#include <ssplayer_wgpu.h>

ss_scene_desc_t desc = {
    .window      = { .kind = SS_WINDOW_METAL_LAYER, .window = my_layer },
    .backend     = SS_BACKEND_AUTO,       /* または SS_BACKEND_DX12 / _METAL / _VULKAN */
    .width       = 960, .height = 540,    /* ポイントではなくピクセル */
    .clear_color = { 0.1f, 0.1f, 0.12f, 1.0f },
    .enable_mask = true,                  /* マスクパーツが効くよう深度/ステンシルを確保 */
    .y_down      = false,
};

ss_scene_t *scene = ss_scene_create(&desc);
if (!scene) { fprintf(stderr, "%s\n", ss_last_error()); return 1; }

ss_player_t *player = ss_scene_create_player(scene);
if (ss_player_load(player, "assets/overall/Basic.ssab") != SS_OK) {
    fprintf(stderr, "%s\n", ss_last_error());
}
ss_player_set_animation(player, "Basic");
ss_player_play(player);

/* ... 毎フレーム、この順で： */
ss_scene_update(scene, delta_seconds);   /* 全プレイヤーを進めてジオメトリを構築 */
ss_scene_render(scene);                  /* 1 パスで全員描画して present */

/* ... リサイズ時（描画ピクセル）： */
ss_scene_resize(scene, width, height);

/* ... 終了時。シーンを壊すとそのプレイヤーも壊れます。 */
ss_scene_destroy(scene);
```

毎フレームの API はこれだけです：**`ss_scene_update` のあと `ss_scene_render`**。1 シーン＝1 ウィンドウ、
1 プレイヤー＝1 キャラクター。シーンは自分が持つ全プレイヤーを 1 パスで、`ss_player_set_order` の昇順に描画します。

1 ウィンドウに 2 体なら `ss_scene_create_player` を 2 回呼ぶだけ。デバイスもパイプラインもアトラス
アップロードも共有されます。`ss_scene_t` と `ss_player_t` が別の型なのはそのためです。

## エラー

失敗しうる関数は `int32_t` を返します：`SS_OK`（0）、`SS_ERROR_INVALID_ARGUMENT`（-1）、
`SS_ERROR_GPU`（-2）。`ss_last_error()` は呼び出しスレッドで最後に起きた失敗の説明を返します
（ログ向けであり、分岐用ではありません）。

ロードは `.ssab` 本体が読めた限り `SS_OK` を返します。**それ以外に届かなかったものは**エラーではなく
`ss_player_missing_count` / `ss_player_missing_at` から返ります
（[ロードとアセット](../guide/loading.md)）。

## サンプルを動かす

プラットフォームごとに 1 つずつ、いずれもこの API をそのまま叩いています：

```bash
cd examples/viewer-metal-cpp   && cmake -B build && cmake --build build && ./build/ssviewer-metal
cd examples/viewer-dx12-cpp    # cmake -B build; cmake --build build --config Release
cd examples/viewer-vulkan-cpp  && cmake -B build && cmake --build build && ./build/ssviewer-vulkan
```

`cargo` はサンプル側が呼ぶので、別途ビルドする必要はありません。アニメーションデータのために
`./scripts/deploy-examples.sh` を先に 1 回だけ実行してください。

!!! note "Linux サンプルはまだ実行されていません"
    ライブラリは Linux（x86_64 / aarch64）向けにビルドされ、リリースにも含まれます。ただし
    **Vulkan サンプルホスト**は macOS 上で書かれたまま一度もビルド・実行されておらず、X11 / Wayland
    どちらの経路も未検証です（[ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md)）。

## 次に読む

* [C API リファレンス](../reference/c.md) — 84 個のエントリポイントを用途別に
* [再生と再生制御](../guide/playback.md) — `play` と `resume` の違い、ループ、区間
* [ロードとアセット](../guide/loading.md) — 自前でテクスチャを供給する方法、`missing` の意味
