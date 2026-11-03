---
title: JavaScript API リファレンス
---

# JavaScript API リファレンス

パッケージは型定義（`dist/index.d.ts`）を同梱しているので、以下はエディタが補完してくれます。このページは見取り図です。

```js
import { sharedContext, restoreSharedDevice, setWasmUrl } from '@spritestudio/ssplayer-wgpu';
import { SsPlayerWebGPUElement, defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';
```

`read` は `SsAssetReader` です：`(url: string) => SsAssetBytes | Promise<SsAssetBytes>`。`SsAssetBytes` は `ArrayBuffer | Uint8Array | Blob | null | undefined`。この 2 つの型と、要素のクラス `SsPlayerWebGPUElement` は export されています。`SsContextWeb.create()` は独自のコンテキスト（= 2 つ目のデバイス）を作るので、呼ぶのは `sharedContext()` にしてください。

要素は独立したエントリポイントにあるので、API だけを使うバンドルに要素は含まれません。

## モジュール関数

| | |
|---|---|
| `sharedContext(): Promise<SsContextWeb>` | ページに 1 つのコンテキスト（デバイス・パイプライン・アトラスキャッシュ）。何度呼んでも同じものが返ります |
| `restoreSharedDevice(ctx): Promise<void>` | デバイスロスト後、新しいデバイス上ですべてを再構築します |
| `setWasmUrl(url)` | 最初の `sharedContext()` より前に wasm バイナリの位置を明示します |
| `defineSsPlayerWebGPUElement(tag?)` | `<ss-player-webgpu>` を登録します。名前の衝突を避けるなら `tag` の名前で登録します。import の副作用ではなく opt-in |

## `SsContextWeb` — ページ

| | |
|---|---|
| `createSurface(canvas)` | `<canvas>` ごとに 1 つ |
| `addTexture(key, rgba, width, height)` | 生の **straight-alpha** RGBA8 でアトラスを供給（`textureRefs` の `key` で） |
| `addTextureFromBytes(key, bytes): Promise` | エンコード済みバイト列から（ブラウザがデコード） |
| `addTextureFromBitmap(key, imageBitmap)` | 既にある `ImageBitmap` から。**premultiplied** でなければなりません（`createImageBitmap(blob, { premultiplyAlpha: 'premultiply' })`） |
| `hasTexture(key)` | キャッシュに既にあるか |
| `isDeviceLost()` / `restoreDevice()` | [デバイスロスト](../guide/device-loss.md)を参照 |
| `surfaceCount()` | 診断用 |
| `version` / `sdkVersion` | この Player のバージョンと、リンクしている SpriteStudio-SDK のバージョン。後者はリリースのタグの形（`v7.0.0`）で、`scripts/build.sh` を通さないビルドなら `local` です。読み取り専用。カスタム要素からは `element.ctx` で届く |

`addTexture`・`addTextureFromBitmap`・`setCellmapTexture` は、デバイスが保持できないテクスチャに対して何も変えずに例外を投げます。幅か高さが `max_texture_dimension_2d` を超える場合、閉じた `ImageBitmap` の場合、`addTexture` で `rgba` が `width * height * 4` バイトでない場合です。`FromBytes` 版も同じ理由で reject します。

`ImageBitmap` から供給したテクスチャ（`addTextureFromBitmap`・`setCellmapTexture`・`setCellmapTextureFromBytes`）は、デバイスロストをまたいで保持されません。`restoreSharedDevice` の後でもう一度供給（適用）してください。`addTexture`・`addTextureFromBytes` で渡したものと、パック自身のアトラスは自動で復元されます。

## `SsSurfaceWeb` — キャンバス 1 枚

| | |
|---|---|
| `createPlayer()` | キャラクターを追加 |
| `frame(dt)` | このキャンバスの全プレイヤーを更新し、1 パスで描画。`dt` は**秒** |
| `resize(width, height)` | 描画ピクセル |
| `setGroupAlpha(on)` / `groupAlpha()` | フェード中のプレイヤーをそれぞれ 1 枚のレイヤーとしてフェード。[キャラクター全体の tint とアルファ](../guide/part-overrides.md#whole-character-tint-and-alpha)を参照 |
| `stats()` | `{ players, prims, drawCalls, uploadedBytes, mergeableDraws, redundantTextureBinds, groupLayers, gpuPassTimeNs? }` |
| `playerCount()` | 診断用 |
| `destroy()` | 明示的かつ冪等。スワップチェーンと**キャンバス上の全プレイヤー**を解放します。JS 参照を捨てるだけではスワップチェーンが残ります |

## `SsPlayerWeb` — キャラクター 1 体

**ロード** — プレイヤーで非同期なのは `load` 系と `setCellmapTextureFromBytes` だけで、1 つのプレイヤーが同時に走らせられるロードは **1 本**です。前のロードが完了する前に次を始めると reject されるので、`await` するか呼び出しを直列につないでください。

| | |
|---|---|
| `load(src, read?): Promise` | URL からパック・アトラス・兄弟パックを。`read(url)` を渡すと `fetch` の代わりにそれが答えます（[ブラウザでは](../guide/loading.md#in-a-browser)） |
| `loadSsab(bytes, pack?, baseUrl?, read?): Promise` | バイト列を登録して選択対象にし、アトラスと兄弟パックも読み込む（`baseUrl` を基準に解決） |
| `addSsab(bytes, pack?, baseUrl?, read?): Promise` | 選択はせずに同じことを行う（インスタンスパーツが見つけられるように） |
| `missingAssets()` | 届かなかったもの。`{ kind: 'texture' \| 'pack', name, slot }[]`。**reject ではありません** |
| `textureRefs(pack?)` / `embeddedTextures(pack?)` | パックが要求するアトラス。`{ key, path, embedded }[]`（`path` は `.ssab` の URL からの相対）/ `{ key, bytes }[]` |
| `animationNames()` / `partNames()` / `cellmapNames()` / `cellNames(cellmap)` | パックが持つもの |
| `currentAnimation()` | 設定中のアニメーション。無ければ `undefined` |
| `canvasSize()` | オーサリング時のキャンバス。`[width, height]`。アニメーション設定前は空の配列 |

**再生制御** — [再生と再生制御](../guide/playback.md)を参照。

| | |
|---|---|
| `setAnimation(name)` | 選択。同期で、アロケーションを伴う呼び出し |
| `play()` / `playFrom(frame)` | **先頭出し** — 巻き戻してループ回数を張り直します |
| `pause()` / `resume()` / `stop()` | 保持、その場から継続、終了 |
| `seek(frame)` / `frameNo()` | 小数フレーム可 |
| `setSpeedScale(s)` / `speedScale()` | 既定 fps の倍率。`0` と負値は停止 |
| `setLoopCount(n)` / `loopCount()` / `remainingLoops()` | `n` 回。`-1` / `0` で無限 |
| `setAnimationSection(start, end)` | `setAnimation` の後に。両端が範囲内で終わりが始まりより前なら、何も変えずに例外を投げる |
| `setPlaybackDirection(dir, style)` / `playbackDirection()` / `playbackStyle()` | それぞれ `0` / `1` |
| `setFrameRate(fps)` / `frameRate()` | 既定 fps を置き換える |
| `setSubFrameEnabled(on)` / `subFrameEnabled()` | フレーム間を補間 |
| `setFrameSkipEnabled(on)` / `frameSkipEnabled()` | 長い tick で複数フレームを跨ぐ |
| `startFrame()` / `endFrame()` / `totalFrames()` | 現在の範囲 |
| `isPlaying()` / `isPausing()` / `isPlayingForward()` / `isFinished()` | 状態 |
| `justStarted()` / `justLooped()` / `justFinished()` | 直前の `frame()` のエッジ。`justStarted` は `resume()` では立ちません |

**配置**

| | |
|---|---|
| `setTransform(x, y, scaleX, scaleY, rotation)` / `resetTransform()` | 位置（キャンバスの中心からのピクセル、`y` は下向きが正）・スケール・回転（ラジアン）。呼ばなければ中央に等倍で置かれます |
| `fitInto(width, height, options?)` | アニメーションを箱に収め、そのトランスフォームを書き込みます。戻り値は `{ x, y, scaleX, scaleY }`。未知の `fit` / `align` は例外になります。[ボックスへのフィット](../guide/placement.md)を参照 |
| `canvasRect()` | `frame` を渡さないときに `fitInto` が収める箱。`{ x, y, width, height }`、`setAnimation` 前は `undefined` |
| `setOrder(n)` / `order()` | キャンバス内の描画順（昇順） |
| `setVisible(v)` / `isVisible()` | 非表示のキャラクターは描画されませんが、再生は続きイベントも出します |
| `setCulled(c)` / `isCulled()` | カリング中のキャラクターは**進みません**。イベントも出しません |
| `setAlpha(a)` / `setTint(rgb)` | キャラクター全体。`alpha()` / `tint()` で読み出せます |
| `setCrowd(placements, tints?)` / `crowdCount()` | キャラクターを多数の配置に描きます。`Float32Array` にメンバー 1 体あたり 6 つの数値（`x, y, scaleX, scaleY, rotation, alpha`）、省略可能な `Uint32Array` にメンバー 1 体あたり 1 つの `0xRRGGBB`。空の配列なら 1 体だけ。[群衆描画](../guide/crowds.md)を参照 |
| `destroy()` | 明示的かつ冪等 |

**パーツ・オーバーライド・イベント** — 各オーバーライドには `ByIndex` 版があります。
`findPartIndex(name)` で一度だけ引いてください。

| | |
|---|---|
| `setPartVisibilityOverride(part, hide, cascade)` / `clearPartVisibilityOverride(part)` | |
| `setPartColorOverride(part, rgba, blendOp, priority)` / `setPartColorOverrideCorners(...)` / `clearPartColorOverride(part)` | `blendOp` 0–3、`priority` 0–2 |
| `setPartCellOverride(part, cellmap, cell, priority)` / `clearPartCellOverride(part)` | |
| `clearAllPartOverrides()` | |
| `partTransform(part)` / `partTransformByIndex(i)` | `[m00, m01, m10, m11, tx, ty]`。不明なパーツなら空の配列（`null` ではない） |
| `isPartHidden(part)` / `isPartSkinnedMesh(part)` / `partCount()` | `isPartHidden` と `isPartSkinnedMesh` は `true`/`false`、不明なパーツなら `undefined` |
| `isPartHiddenByIndex(i)` / `isPartSkinnedMeshByIndex(i)` | 同じことをパーツインデックス（イベントの `partIndex`、`partNames()` の位置）で。範囲外なら `undefined` |
| `events()` | `kind` で分類されたプレーンオブジェクト：`'userData'` / `'signal'` / `'audio'`。どれも `frameNo`・`partName` と、発火したソースのハンドル `source` を持つ |

**テクスチャ差し替え** — セルマップがバインドするテクスチャ丸ごとを差し替えます（衣装・パレット変更）。`pack` に null でメインパック。[テクスチャ差し替え](../guide/texture-replacement.md)を参照。

| | |
|---|---|
| `replaceableCellmapNames(pack?)` | 差し替え可能な名前：セルマップと TextureChange の差し先の両方 |
| `setCellmapTexture(pack, name, imageBitmap)` | **premultiplied** な `ImageBitmap` から差し替え |
| `setCellmapTextureFromBytes(pack, name, bytes): Promise` | …またはエンコード済みバイト列から（ブラウザがデコード） |
| `restoreCellmapTexture(pack, name)` | 元のアトラスに戻す |
| `isCellmapOverridden(pack, name)` | 現在オーバーライドされているか |

### モーションブレンディング { #motion-blending }

トラックスタックです。トラックはセットアップポーズの上に昇順に重なり、トラック 0 は `setAnimation` が確立するものです。[モーションブレンディング](../guide/motion-blending.md)を参照。

| | |
|---|---|
| `setTrackAnimation(track, name)` | トラックにアニメーションを載せる（それまでの再生内容を置き換える） |
| `crossfade(track, name, ms)` | トラックを別のアニメーションへフェードで切り替える。トラック 0 では、呼んだ時点から `currentAnimation()` が替わる |
| `fadeTrack(track, target, ms)` / `cancelTrackFade(track)` | トラックの alpha を目標値へ動かす／フェードをその場で止める |
| `trackFadeProgress(track)` | 0〜1。フェードしていないときは `undefined` |
| `setTrackAlpha(track, a)` / `trackAlpha(track)` | トラックの強さ |
| `setTrackBlend(track, b)` / `trackBlend(track)` | `0` が mix、`1` が additive |
| `setTrackSync(track, s)` / `trackSync(track)` | `0` が free、`1` が start。載せる前に設定する |
| `setTrackFrame` / `trackFrame` / `setTrackSpeed` | トラックのスクラブ。速度 0 で止まる |
| `trackSpeed` / `setTrackFrameRate` / `trackFrameRate` / `setTrackFrameSkipEnabled` / `trackFrameSkipEnabled` | トラックごとの再生設定。トラック 0 の設定は再生制御の呼び出し |
| `setTrackPlaybackDirection(track, direction, style)` / `trackPlaybackDirection` / `trackPlaybackStyle` | `setPlaybackDirection` と同じ `0` / `1` |
| `setTrackLoopCount` / `trackLoopCount` | トラックのアニメーションのループ回数。載せた後で設定する。`-1` は無限 |
| `isTrackFinished` / `trackJustLooped` | トラックの終了（状態）とループ（その update だけ） |
| `setTrackOutgoingEventThreshold` / `trackOutgoingEventThreshold` | クロスフェードの出ていく側が、どこまでイベントを発火し続けるか。既定の `Infinity` では発火しない |
| `trackCount()` / `clearTrack(track)` | トラックの数／トラックを空にする（トラック 0 は不可） |
| `trackMainSource(track)` | トラックのアニメーションを担うソース |
| `addTrackSource(track, name, weight)` | 別のアニメーションを横に載せ、weight で混ぜる。ハンドルを返す |
| `setSourceWeight` / `setSourceFrame` / `setSourceEventThreshold` と、`set` の無い取得 | ソース 1 つの weight・再生位置・イベントのしきい値 |

存在しないトラックやソースを読むと `undefined` が返ります（`isTrackFinished` と `trackJustLooped` は `false`）。操作すると例外を投げます。

## `<ss-player-webgpu>` { #ss-player-webgpu }

詳細は[専用ページ](./web-component.md)にあります。属性だけ再掲すると：

| 属性 | 意味 |
|---|---|
| `src` | `.ssab` の URL |
| `pack` | パック名の上書き |
| `animation` | 再生するアニメーション。既定は最初のもの |
| `autoplay` | boolean 属性 — **無ければ再生せず**、1 フレーム目で止まります |
| `pause-when-hidden` | 既定でオン。`"false"` で解除 |
| `speed-scale` | 既定 fps の倍率 |
| `fit`、`align`、`flip-x`、`flip-y` | オーサリング時のキャンバスを要素にどう合わせるか。既定は `contain` と `center`、`fit="none"` で 1:1 |

`element.loadSrc(src, pack?, animation?, read?)` でスクリプトからパックを差し替えられ、アニメーション名が返ります。`animationstarted`・`animationfinished`・`animationlooped`・`userdata`・`signal`・`audio` を DOM イベントとして送り、パックを読み込むと `load` を送ります。`src`・`pack`・`animation`・`speed-scale` と配置の属性は、置いた後に書き換えても反映されます。`autoplay` はアニメーションを設定するときに、`pause-when-hidden` は要素の接続時に 1 回だけ読みます。モーションブレンディングを含め、`SsPlayerWeb` の機能はすべて `element.player` から使えます。
