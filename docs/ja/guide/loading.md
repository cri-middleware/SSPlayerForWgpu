# ロードとアセット

**ロードはどの Face でも 1 呼び出しです。** パック本体、どちらのエクスポートモードのアトラスも、そしてインスタンスパーツが再生する兄弟 `.ssab` まで再帰的に解決します。テクスチャを自分で集める必要はありません。

=== "Rust"

    ```rust
    let mut player = SSPlayer::new();
    player.load("assets/overall/Instance.ssab")?;   // 依存するものすべて
    player.set_animation("anime_1")?;
    player.play();
    ```

=== "C"

    ```c
    ss_player_t *player = ss_scene_create_player(scene);
    ss_player_load(player, "assets/overall/Instance.ssab");
    ss_player_set_animation(player, "anime_1");
    ss_player_play(player);
    ```

=== "JavaScript"

    ```js
    const player = surface.createPlayer();
    await player.load('/assets/Instance.ssab');   // 唯一の非同期呼び出し
    player.setAnimation('anime_1');
    player.play();
    ```

=== "Bevy"

    ```rust
    commands.spawn(SsPlayer {
        pack: asset_server.load("Ringo/Ringo.ssab"),
        animation: "wait1".into(),
        ..default()
    });
    ```

## 1 回のロードが実際に取ってくるもの

3 種類あり、2 番目が事故のもとです：

1. **パック本体**（指定したパス／URL から）。
2. **両方のテクスチャテーブルにあるアトラス。** ドローバッチのテクスチャキーは、通常のパーツではセルマップの名前ハッシュですが、アーティストが**テクスチャチェンジ**（`TCHG`）を設定したパーツでは*テクスチャファイル名*のハッシュです。セルマップだけを読むホストは 2 つ目を取りこぼし、しかもそれは静かに壊れます：1×1 の白テクスチャにフォールバックし、白いシルエットで描かれるだけで、エラーも警告もカウンタも動きません。
3. **インスタンスパーツ用の兄弟パック** — 別キャラクターのアニメーションを内部で再生するもの。
   `<パック自身のディレクトリ>/<名前>.ssab` として解決されます。

同じファイルが 2 つのハッシュで 2 回名指しされることがあります。これは正常です。フェッチの重複排除は**ハッシュではなくパスで**行ってください。

## パスは `.ssab` 基準で解決されます

`load` に渡すのはパス（URL）1 つです。以降パックが名指しするものはすべて、**そのファイルが実際に置かれていたディレクトリ**を基準に解決されます。エクスポータがそのように相対パスを書いているためです。

つまり：エクスポートしたパックとアトラスは同じ場所に置き、渡すのは `.ssab` のパス（ディレクトリでもアトラスでもなく）にしてください。

## `load` が返った時点で GPU には何もありません

借用モデルではフレームが始まるまでデバイスが貸し出されないため、ロードはバイト列を読むところで止まります。読んだものは**最初の `update`** でデコードされ、アップロードされます。これは埋め込みアトラスが従来からたどってきたタイミングとまったく同じです。

そのため、ロード直後の時点ではレンダラーはまだテクスチャのサイズを知りません。最初のフレームより後に問い合わせてください。

## 届かなかったもの：エラーではなく `missing`

**ロードが失敗するのは `.ssab` 本体が読めなかったときだけです。** アトラス 1 枚の欠落でキャラクター全体を失ってはいけないので、それ以外に届かなかったものは、分岐に使えるリストとして返ります。

=== "Rust"

    ```rust
    player.load("assets/overall/Instance.ssab")?;   // Err は .ssab 本体が読めなかった場合のみ
    for missing in player.missing_assets() {
        eprintln!("missing {:?}: {}", missing.kind, missing.name);
    }
    ```

=== "C"

    ```c
    if (ss_player_load(player, path) != SS_OK) {
        fprintf(stderr, "%s\n", ss_last_error());
    } else if (ss_player_missing_count(player) > 0) {
        SsMissingAsset m;
        for (uint32_t i = 0; i < ss_player_missing_count(player); i++) {
            if (ss_player_missing_at(player, i, &m) == SS_OK) {
                fprintf(stderr, "missing: %s\n", m.name);
            }
        }
    }
    ```

=== "JavaScript"

    ```js
    await player.load('/assets/Instance.ssab');   // reject するのは .ssab 本体だけ
    for (const m of player.missingAssets()) {
      console.warn('missing', m.kind, m.name);
    }
    ```

開発中に一度は確認してください。「ロードは通るのに白いシルエットで描かれる」はほぼ確実にここのエントリが原因です。

## すでにバイト列を持っている場合

自分でファイルを取得したホスト（ドラッグ＆ドロップ、バンドラ、アーカイブ、エンジンのアセットシステム）は、バイト列を直接登録します。アトラスや兄弟パックを `.ssab` の隣に置いたフォルダごとドロップされた場合は、リーダーを渡して `load` 1 回で読むほうが向いています。[ブラウザでは](#in-a-browser)を参照してください。

=== "Rust"

    ```rust
    player.load_ssab("Character", &bytes)?;   // 登録し、選択対象のパックにする
    player.add_ssab("Effect", &other)?;       // 登録のみ（インスタンスパーツが見つけられるように）
    ```

=== "C"

    ```c
    ss_player_load_ssab(player, "Character", data, len);
    ss_player_add_ssab(player, "Effect", other, other_len);
    ```

=== "JavaScript"

    ```js
    await player.loadSsab(bytes, 'Character', new URL('/assets/Character.ssab', location.href).href);
    await player.addSsab(other, 'Effect', baseUrl);
    ```

兄弟パックをロード**前**に登録しておくこともできます。既に持っているパックは走査がスキップします。

!!! warning "再登録は、いま再生中のものを変えません"
    再生ツリーは、それを構築したパックに紐づいています。再生中のツリーが使っている名前で新しいパックを登録すると、古いパックは破棄されずに退役します。画面上のものはそのまま再生を続け、差し替えたものは**次の** `set_animation` が構築の元にします。

## テクスチャを自前で供給する { #supplying-textures-yourself }

画像をホスト側が持っている場合（アセットシステムを備えたエンジン、自作アトラス、既に済ませたフェッチ）は、**セルマップではなく画像を指すキー**で渡します。テクスチャマップは**デバイススコープ**なので、同じパックを
5 体で表示してもアップロードは 1 回です。

!!! warning "セルマップのハッシュが指すのはスロットであって画像ではありません"
    Adobe Animate はどのプロジェクトを書き出しても `spritemap1.png` という名前を付けるため、別プロジェクトの
    2 つのパックが同じセルマップハッシュを要求しながら別のアトラスを意味します。そのハッシュで登録すると
    2 つ目のパックは「もう供給済み」に見え、自分のピクセルは捨てられてパーツが 1 つ目のアトラスを描きます。キーは Player に聞いてください。画像そのものを指す値が返るので、本当に同じファイルを共有している
    2 パックは今まで通り 1 回のアップロードで済みます。

=== "Rust"

    ```rust
    // `texture.hash` はスロットの名前。レンダラのキーはこちら。
    let key = player.texture_key(pack, texture.hash);

    // あなたが所有する view。Player が GPU テクスチャを所有することはありません。
    renderer.set_texture(&device, key, &view, (width, height), AlphaMode::Straight);

    // ストレートアルファの RGBA8 からアップロードさせることもできます。
    renderer.upload_texture(&device, &queue, key, &rgba, (width, height), Retention::Keep);
    ```

=== "C"

    ```c
    /* `SsTextureRef::hash` は既にキーです（`ss_player_texture_at` が解決して返します） */
    /* 生の RGBA8、width * height * 4 バイト、ストレートアルファ */
    ss_scene_add_texture(scene, ref.hash, rgba, width, height);

    /* あるいはエンコード済み PNG を渡してデコードさせる */
    ss_scene_add_texture_png(scene, ref.hash, png_bytes, png_len);
    ```

=== "JavaScript"

    ```js
    // textureRefs / embeddedTextures の各エントリは `hash` の隣に `key` を持ちます。
    ctx.addTexture(key, rgba, width, height);          // 生の RGBA8
    await ctx.addTextureFromBytes(key, encodedBytes);  // PNG/WebP。ブラウザがデコード
    ctx.addTextureFromBitmap(key, imageBitmap);        // 既にある ImageBitmap
    ```

*何を*供給すべきかはパックに聞きます。各エントリは、アトラスが `.ssab` の中にあるか（embedded — 何もしなくてよい）、エクスポータが**パック自身のディレクトリからの相対**で書いたファイル名かを教えてくれます：

| | Rust | C | JavaScript |
|---|---|---|---|
| 列挙 | `player.textures(pack)` | `ss_player_texture_count` / `ss_player_texture_at` | `player.textureRefs(pack)` |
| 埋め込みバイト列 | `player.embedded_images(pack)` | *(呼び出し不要)* | `player.embeddedTextures(pack)` |

## `File::open` が届かない場所から読む

エクスポートした Godot プロジェクトは `.pck`、Android ビルドは APK、Unreal は `.pak`、ブラウザは `fetch` の向こう側にファイルを持っています。Rust ホストは `load_with` で自前のリーダーを同じ走査に差し込めます：

```rust
player.load_with(&mut my_loader, "res://characters/Ringo.ssab").await?;
```

`AssetLoader` は 6 メソッド（`reset_base` / `set_base` / `read` / `supply_texture` / `has_texture` / `warn`）で、走査側の契約は固定されテストされています：

1. まず `reset_base()`。1 つのローダーで何度でもロードできるようにするため；
2. `read` には `src` が**そのまま**渡ります。走査はパスを組み立てず、`src` の中身を覗きません；
3. *その後で* `set_base(src)` が 1 回だけ呼ばれ、以降のパスはすべてそこからの**裸の名前**です。

「読んでから base を張る」順序が安全性の要です。`src` はそれ由来の base が存在するより前に読まれるので、「`src` を `src` 自身に対して解決する」という状態が表現できません。解決するのはあなたの仕事です。手元にあるのがファイルパスなのか URL なのかアーカイブのキーなのかを知っているのはあなただけだからです。

!!! note "代わりに `std::fs` を使わないでください"
    wasm32 でもコンパイルは通り、実行時に失敗します。アーカイブ内のファイルも開けません。この継ぎ目はまさにその 2 つのために存在します。

### ブラウザでは { #in-a-browser }

Web フェイスは URL の解決を自分で行うので、差し替えるのは `fetch` だけです。`load` に `read(url)` を渡すと、走査が求めるファイル（`.ssab`、そのアトラス、兄弟パック）はすべてそちらから取られます。渡されるのは、fetch なら使ったはずの絶対 URL です。`ArrayBuffer`・`Uint8Array`・`Blob` のどれかを返し、持っていないファイルには `null` を返します。

ドロップされたフォルダはこの形で再生します。中身を自分で決めた URL の下に並べ、そこから読み込みます。

```js
// ドロップごとに base を分ける。アトラスは URL で識別されるので、
// 同じ base に 2 つ目のフォルダを置くと、同名のアトラスが読み込み済みと見なされ、1 つ目の絵で描かれる。
const base = new URL(`dropped/${++drops}/`, location.href);
const files = new Map(dropped.map(({ path, file }) => [new URL(path, base).href, file]));
await player.load(new URL('Ringo/Ringo.ssab', base).href, (url) => files.get(url));
```

`loadSsab`・`addSsab`・要素の `loadSrc` も、最後の引数に同じ `read` を取ります。<a href="../../demo/" target="_blank" rel="noreferrer">WebGPU デモ</a>は、ドロップされたフォルダをこの方法で再生しています。

## 次に読む

* [マスク](./masks.md) — 描画のうち唯一、描画ターゲットに依存するもの
* [デバイスロスト](./device-loss.md) — GPU が消えたとき、上記すべてがどうなるか
* [アセットの準備](../start/assets.md) — そもそも `.ssab` を作る話
