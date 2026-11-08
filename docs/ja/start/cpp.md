---
title: C / C++ ではじめる
---

# C / C++ ではじめる

静的ライブラリ 1 本とヘッダ 1 枚。ウィンドウ（あるいはレイヤー、サーフェス）を渡して毎フレーム
2 つの関数を呼ぶと、**Windows では DirectX 12、macOS では Metal、Linux では Vulkan** で描画されます。

!!! tip "wgpu を知っている必要はありません"
    wgpu は「1 つのライブラリで 3 つのグラフィックス API を賄う」ための手段であり、ここでの関わりはそれだけです。**wgpu の型はこの ABI を一切越えず**、ヘッダにも登場しません。渡すのはプラットフォームのウィンドウハンドルで、返ってくるのは `SS_OK` かエラーコードです。

    1 点だけ：この Face は**自分でグラフィックスデバイスを持ちます**。あなたの
    `ID3D12CommandList` や `MTLCommandBuffer` に描き込むわけではありません。エンジンのデバイスを C ABI 経由で共有することは[意図的に対象外](../limitations.md#no-device-sharing-across-the-c-abi)です。また、シーンが描くのは作成時に渡したウィンドウだけで、エンジン側で合成するためのテクスチャへは描きません。

## 手に入るもの

| | |
|---|---|
| `include/ssplayer_wgpu.h` | ヘッダ 1 枚。C99、依存なし。cbindgen が生成 |
| `lib/libssplayer_wgpu.a`（MSVC では `ssplayer_wgpu.lib`） | 静的ライブラリ |
| `lib/libssplayer_wgpu.dylib`（macOS のみ） | 共有ライブラリ。署名・公証済み。ライブラリをリンクせずに読み込むホスト向け |
| `lib/VERSION` | ライブラリがリンクしている SpriteStudio-SDK のバージョン（リリースのタグの形）。`ss_sdk_version()` が返すものと同じ |
| `LICENSE.md` | BSD-3-Clause |
| `LICENSE.ja.md` | その参考訳 |
| `THIRD_PARTY_NOTICES.md` | ライブラリに組み込まれた Rust クレートのライセンス。謝辞に載せるべきもの |
| `README.md`、`docs/` | このドキュメントの Markdown（英語）。zip と同じリリースのもので、オフラインで読める |

## 導入 { #install }

=== "リリース版"

    各 [`ssplayer-wgpu` の Release](https://github.com/cri-middleware/SSPlayerForWgpu/releases?q=ssplayer-wgpu&expanded=true) の **Assets** に、プラットフォームごとの zip（`include/` `lib/` `LICENSE.md` `LICENSE.ja.md` `THIRD_PARTY_NOTICES.md` `README.md` `docs/`）が付いています：

    | zip | 対象 |
    |---|---|
    | `ssplayer-wgpu-windows-x86_64.zip` | Windows / MSVC（DX12） |
    | `ssplayer-wgpu-macos-universal.zip` | macOS。arm64 + x86_64 が 1 本に（Metal） |

    **静的ライブラリをリンクします**。自分のバイナリに組み込まれるので、横に置いて配布するランタイム依存はありません。macOS の zip には `lib/libssplayer_wgpu.dylib` も入っています。Python・.NET・Dart の FFI のように、ライブラリをリンクせずに読み込むホスト向けです。Developer ID で署名・公証してあるので、ダウンロードしたものを macOS がそのまま読み込みます（初回は Gatekeeper がチケットをオンラインで確かめます）。install name は `@rpath/libssplayer_wgpu.dylib` です。Windows で DLL が必要な場合はソースからビルドしてください（`build.ps1` が生成します）。Linux でもソースからビルドします。Linux 向けのライブラリはリリースの zip に含まれません。

    Windows のライブラリは、C ランタイムを DLL 版（`/MD`）でリンクしています。CMake と Visual Studio の既定と同じです。静的ランタイム（`/MT`）でビルドするホストは、静的ランタイムを有効にしてソースからビルドしてください（「ソースから」を参照）。

=== "ソースから"

    必要なのは Rust ツールチェイン（1.99+）だけです。同じビルドで cbindgen がヘッダも再生成するため、ヘッダは常に今ビルドしたライブラリと一致します。

    ```bash
    git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
    cd SSPlayerForWgpu
    cargo build --release -p ssplayer-wgpu-ffi
    ```

    | 場所 | 内容 |
    |---|---|
    | `target/release/` | `libssplayer_wgpu.a` **と** `libssplayer_wgpu.dylib` / `.so`。MSVC では `ssplayer_wgpu.lib` / `ssplayer_wgpu.dll` / インポートライブラリ `ssplayer_wgpu.dll.lib` |
    | `libs/ssplayer-wgpu-ffi/include/ssplayer_wgpu.h` | このビルドで再生成されたヘッダ |

    `./scripts/build.sh`（Windows では `build.ps1`）も同じことをしつつ、結果を `bin/` に収集します（`bin/include/`、`bin/libs/<platform>/<arch>/`）。これはリリース zip の元になるレイアウトです。
    macOS で 2 スライスのライブラリが欲しい場合は `arch=universal` を付けます。

    静的 C ランタイム（`/MT`）でビルドする Windows のホストに向けては、ライブラリも同じランタイムでビルドします。

    ```powershell
    $env:RUSTFLAGS = "-C target-feature=+crt-static"
    cargo build --release -p ssplayer-wgpu-ffi
    ```

## リンク

ライブラリは Rust 製なので、**静的**リンクにはツールチェインが本来指定してくれるプラットフォームライブラリが必要です。対象プラットフォーム上でツールチェインに聞いてください：

```bash
cargo rustc -p ssplayer-wgpu-ffi --release --crate-type staticlib -- --print native-static-libs
```

macOS ではこう返ります：`-lSystem -framework QuartzCore -framework CoreGraphics -liconv -framework Metal -framework Foundation -framework CoreFoundation -lobjc`（出力の残りはこれらの繰り返しです）。これに自分のウィンドウコードが必要とするもの（`-framework Cocoa`）を足します。Windows では `opengl32.lib kernel32.lib ntdll.lib userenv.lib ws2_32.lib dbghelp.lib /defaultlib:msvcrt` が返るので、リリースの `ssplayer_wgpu.lib` と一緒にリンクします。macOS の CMake なら（`SSPLAYER_DIR` は、リリースの zip を展開したフォルダです）：

```cmake
add_executable(myapp src/main.cpp)
target_include_directories(myapp PRIVATE "${SSPLAYER_DIR}/include")
target_link_libraries(myapp PRIVATE
    "${SSPLAYER_DIR}/lib/libssplayer_wgpu.a"
    "-framework Cocoa" "-framework QuartzCore" "-framework CoreGraphics" "-framework Metal"
    "-framework Foundation" "-framework CoreFoundation" objc iconv)
```

リポジトリ内の 3 つのサンプルビューアは「ソースから」の経路を取り、動的ライブラリをリンクしているため上記は不要です。その形は
[`examples/viewer-metal-cpp/CMakeLists.txt`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/examples/viewer-metal-cpp/CMakeLists.txt)
を参照してください。

## ウィンドウの指定

`ss_window_handle_t` はプラットフォームのウィンドウを ABI 用に平坦化したものです。`kind` が使うフィールドだけ埋め、残りは null にします。

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

サンプルが読み込む `.ssab` は事前にコンバートしたものです。`.sspj` からの作り方は[アセットの準備](./assets.md)にあり、公式のサンプルデータなら自前のプロジェクトは要りません。

```c
#include <stdio.h>
#include <ssplayer_wgpu.h>

int main(void) {
    void *my_layer = /* あなたの CAMetalLayer* */ NULL;
    float delta_seconds = 1.0f / 60.0f;
    uint32_t width = 960, height = 540;

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
    ss_player_set_animation(player, "anime_1");
    ss_player_play(player);

    /* ... 毎フレーム、この順で： */
    ss_scene_update(scene, delta_seconds);   /* 全プレイヤーを進めてジオメトリを構築 */
    ss_scene_render(scene);                  /* 1 パスで全員描画して present */

    /* ... リサイズ時（描画ピクセル）： */
    ss_scene_resize(scene, width, height);

    /* ... 終了時。シーンを壊すとそのプレイヤーも壊れます。 */
    ss_scene_destroy(scene);
    return 0;
}
```

例は C99 です。C++20 より前の C++（MSVC の既定は C++14）には指示付き初期化子が無いので、構造体をゼロで初期化して（`ss_scene_desc_t desc{};`）、フィールドを 1 つずつ設定してください。

毎フレームの API はこれだけです：**`ss_scene_update` のあと `ss_scene_render`**。1 シーン＝1 ウィンドウ、
1 プレイヤー＝1 キャラクター。シーンは自分が持つ全プレイヤーを 1 パスで、`ss_player_set_order` の昇順に描画します。

1 ウィンドウに 2 体なら `ss_scene_create_player` を 2 回呼ぶだけ。デバイスもパイプラインもアトラスアップロードも共有されます。`ss_scene_t` と `ss_player_t` が別の型なのはそのためです。

## エラー

失敗しうる関数は `int32_t` を返します：`SS_OK`（0）、`SS_ERROR_INVALID_ARGUMENT`（-1）、`SS_ERROR_GPU`（-2）、または要求を断ったときのランタイム自身の正のコードです。判定は `SS_OK` と比べて行ってください。`ss_last_error()` は呼び出しスレッドで最後に起きた失敗の説明を返します（ログ向けであり、分岐用ではありません）。

ロードは `.ssab` 本体が読めた限り `SS_OK` を返します。**それ以外に届かなかったものは**エラーではなく
`ss_player_missing_count` / `ss_player_missing_at` から返ります（[ロードとアセット](../guide/loading.md)）。

## サンプルを動かす

ホストは 3 つ、プラットフォームごとに 1 つずつで、いずれも単体で完結し、この API をそのまま叩いています：

```bash
cd examples/viewer-metal-cpp   && cmake -B build && cmake --build build && ./build/ssviewer-metal
cd examples/viewer-dx12-cpp    # cmake -B build; cmake --build build --config Release
cd examples/viewer-vulkan-cpp  && cmake -B build && cmake --build build && ./build/ssviewer-vulkan
```

`cargo` はサンプル側が呼ぶので、別途ビルドする必要はありません。アニメーションデータのために
`./scripts/deploy-examples.sh` を先に 1 回だけ実行してください。

## 次に読む

* [C API リファレンス](../reference/c.md) — 全エントリポイントを用途別に
* [再生と再生制御](../guide/playback.md) — `play` と `resume` の違い、ループ、区間
* [ロードとアセット](../guide/loading.md) — 自前でテクスチャを供給する方法、`missing` の意味
