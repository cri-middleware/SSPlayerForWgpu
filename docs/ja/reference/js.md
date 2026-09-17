---
title: JavaScript API リファレンス
---

# JavaScript API リファレンス

パッケージは型定義（`dist/index.d.ts`）を同梱しているので、以下はエディタが補完してくれます。
このページは見取り図です。

```js
import { sharedContext, restoreSharedDevice, setWasmUrl } from '@spritestudio/ssplayer-wgpu';
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';
```

要素は独立したエントリポイントにあるので、API だけを使うバンドルに要素は含まれません。

## モジュール関数

| | |
|---|---|
| `sharedContext(): Promise<SsContextWeb>` | ページに 1 つのコンテキスト（デバイス・パイプライン・アトラスキャッシュ）。何度呼んでも同じものが返ります |
| `restoreSharedDevice(ctx): Promise<void>` | デバイスロスト後、新しいデバイス上ですべてを再構築します |
| `setWasmUrl(url)` | 最初の `sharedContext()` より前に wasm バイナリの位置を明示します |
| `defineSsPlayerWebGPUElement()` | `<ss-player-webgpu>` を登録します。import の副作用ではなく opt-in |

## `SsContextWeb` — ページ

| | |
|---|---|
| `createSurface(canvas)` | `<canvas>` ごとに 1 つ |
| `addTexture(key, rgba, width, height)` | 生の RGBA8 でアトラスを供給（`textureRefs` の `key` で） |
| `addTextureFromBytes(key, bytes): Promise` | エンコード済みバイト列から（ブラウザがデコード） |
| `addTextureFromBitmap(key, imageBitmap)` | 既にある `ImageBitmap` から |
| `hasTexture(key)` | キャッシュに既にあるか |
| `isDeviceLost()` / `restoreDevice()` | [デバイスロスト](../guide/device-loss.md)を参照 |
| `surfaceCount()` | 診断用 |

## `SsSurfaceWeb` — キャンバス 1 枚

| | |
|---|---|
| `createPlayer()` | キャラクターを追加 |
| `frame(dt)` | このキャンバスの全プレイヤーを更新し、1 パスで描画。`dt` は**秒** |
| `resize(width, height)` | 描画ピクセル |
| `stats()` | `{ players, prims, drawCalls, uploadedBytes, mergeableDraws, redundantTextureBinds, gpuPassTimeNs? }` |
| `playerCount()` | 診断用 |
| `destroy()` | 明示的かつ冪等。JS 参照を捨てるだけではスワップチェーンが残ります |

## `SsPlayerWeb` — キャラクター 1 体

**ロード** — 非同期なのは `load` 系だけで、1 つのプレイヤーが同時に走らせられるロードは **1 本**
です。前のロードが完了する前に次を始めると reject されるので、`await` するか呼び出しを直列に
つないでください。

| | |
|---|---|
| `load(src): Promise` | URL からパック・アトラス・兄弟パックを |
| `loadSsab(bytes, pack?, baseUrl?): Promise` | バイト列を登録し、選択対象にする |
| `addSsab(bytes, pack?, baseUrl?): Promise` | 登録のみ |
| `missingAssets()` | 届かなかったもの。**reject ではありません** |
| `textureRefs(pack?)` / `embeddedTextures(pack?)` | パックが要求するアトラス |
| `animationNames()` / `partNames()` / `cellmapNames()` / `cellNames(cellmap)` | パックが持つもの |
| `canvasSize()` | オーサリング時のキャンバス |

**再生制御** — [再生と再生制御](../guide/playback.md)を参照。

| | |
|---|---|
| `setAnimation(name)` | 選択。同期で、アロケーションを伴う呼び出し |
| `play()` / `playFrom(frame)` | **先頭出し** — 巻き戻してループ回数を張り直します |
| `pause()` / `resume()` / `stop()` | 保持、その場から継続、終了 |
| `seek(frame)` / `frameNo()` | 小数フレーム可 |
| `setSpeedScale(s)` | 既定 fps の倍率。`0` と負値は停止 |
| `setLoopCount(n)` / `loopCount()` / `remainingLoops()` | `n` 回。`-1` / `0` で無限 |
| `setAnimationSection(start, end)` | `setAnimation` の後に |
| `setPlaybackDirection(dir, style)` / `playbackDirection()` / `playbackStyle()` | それぞれ `0` / `1` |
| `setFrameRate(fps)` / `frameRate()` | 既定 fps を置き換える |
| `setSubFrame(on)` / `subFrameEnabled()` | フレーム間を補間 |
| `setSkipFrames(on)` / `skipFrames()` | 長い tick で複数フレームを跨ぐ |
| `startFrame()` / `endFrame()` / `totalFrames()` | 現在の範囲 |
| `isPlaying()` / `isPausing()` / `isPlayingForward()` / `isFinished()` | 状態 |
| `justLooped()` / `justFinished()` | 直前の `frame()` のエッジ |

**配置**

| | |
|---|---|
| `setTransform(x, y, scaleX, scaleY, rotation)` / `resetTransform()` | |
| `setOrder(n)` | キャンバス内の描画順（昇順） |
| `setVisible(v)` | 非表示のキャラクターは**進みません**。イベントも出しません |
| `setAlpha(a)` / `setTint(rgb)` | キャラクター全体 |
| `destroy()` | 明示的かつ冪等 |

**パーツ・オーバーライド・イベント** — 各オーバーライドには `ByIndex` 版があります。
`findPartIndex(name)` で一度だけ引いてください。

| | |
|---|---|
| `setPartVisibilityOverride(part, hide, cascade)` / `clearPartVisibilityOverride(part)` | |
| `setPartColorOverride(part, rgba, blendOp, priority)` / `setPartColorOverrideCorners(...)` / `clearPartColorOverride(part)` | `blendOp` 0–3、`priority` 0–2 |
| `setPartCellOverride(part, cellmap, cell, priority)` / `clearPartCellOverride(part)` | |
| `clearAllPartOverrides()` | |
| `partTransform(part)` / `partTransformByIndex(i)` | `[m00, m01, m10, m11, tx, ty]` |
| `isPartHidden(part)` / `isPartSkinnedMesh(part)` / `partCount()` | `isPartSkinnedMesh` は `true`/`false`、不明なパーツなら `null` |
| `events()` | `kind` で分類されたプレーンオブジェクト：`'userData'` / `'signal'` / `'audio'` |

**テクスチャ差し替え** — セルマップがバインドするテクスチャ丸ごとを差し替えます（衣装・パレット
変更）。`pack` に null でメインパック。[テクスチャ差し替え](../guide/texture-replacement.md)を参照。

| | |
|---|---|
| `replaceableCellmapNames(pack?)` | 差し替え可能な名前：セルマップと TextureChange の差し先の両方 |
| `setCellmapTexture(pack, name, imageBitmap)` | **premultiplied** な `ImageBitmap` から差し替え |
| `setCellmapTextureFromBytes(pack, name, bytes): Promise` | …またはエンコード済みバイト列から（ブラウザがデコード） |
| `restoreCellmapTexture(pack, name)` | 元のアトラスに戻す |
| `isCellmapOverridden(pack, name)` | 現在オーバーライドされているか |

## `<ss-player-webgpu>`

詳細は[専用ページ](./web-component.md)にあります。属性だけ再掲すると：

| 属性 | 意味 |
|---|---|
| `src` | `.ssab` の URL |
| `pack` | パック名の上書き |
| `anim` | 再生するアニメーション。既定は最初のもの |
| `autoplay` | boolean 属性 — **無ければ再生せず**、1 フレーム目で止まります |
| `pause-when-hidden` | 既定でオン。`"false"` で解除 |

`element.loadSrc(src, pack?, anim?)` で実行時にパックを差し替えられ、アニメーション名が返ります。

## この Face に無いもの

**モーションブレンディング**（トラックスタック）は Rust だけです。全体像は
[スコープと名前の対応](../guide/concepts.md#what-each-face-can-do)を参照。
