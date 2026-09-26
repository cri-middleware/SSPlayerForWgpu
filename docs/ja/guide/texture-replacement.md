# テクスチャ差し替え

**セルマップ**がバインドするテクスチャ（アトラス丸ごと）を、`.ssab` を触らずにプログラムから差し替えます。これがアバターの着せ替えのための経路です — 衣装変更、パレット差し替え、動的な表情。元と**同じレイアウト**の別アトラスを用意して実行時にバインドすれば、そのセルマップを使うすべてのパーツがそれを反映します。

これは**プレーヤ側のオーバーライド**であり、[パーツオーバーライド](./part-overrides.md)とは別物です — あちらは*1つのパーツ*が描くものを変え、こちらは*テクスチャ丸ごと*を差し替えます。

## 差し替え対象の指定

テクスチャは**セルマップ名**（SpriteStudio の名前、`.ssce` 拡張子なし）か、
**TextureChange の差し先**のファイル名で指定します。どちらも描画が解決に使う名前で、どちらも列挙できるので、ハードコードせずに UI で選択肢を提示できます。パックを指定しなければ**メインパック**が選ばれます。

=== "Rust"

    ```rust
    for name in player.replaceable_cellmap_names(None).unwrap_or_default() {
        // "common", "Clothes", "spritemap1.png" … セルマップと TextureChange の差し先の両方
    }
    ```

=== "C"

    ```c
    uint32_t n = ss_player_cellmap_texture_count(player, /* pack */ NULL);
    for (uint32_t i = 0; i < n; i++) {
        const char *name = ss_player_cellmap_texture_name(player, NULL, i);
    }
    ```

=== "JavaScript"

    ```js
    const names = player.replaceableCellmapNames(null);  // string[]
    ```

=== "Bevy"

    ```rust
    // 名前はパックに由来します。レンダーワールド側プレーヤの Rust API で取得するか、
    // 元の `.ssae` をエディタで確認してください。
    ```

## 差し替えと復元

差し替えテクスチャは、ロード済みアトラスと同じ straight-alpha RGBA8 です。GPU へ渡す途中で
premultiplied 化されます。**復元**はセルマップを元のアトラスに指し直すだけで — 元アトラスには一切触れていないため、再ロードは発生しません。

=== "Rust"

    ```rust
    // メインパックの "Clothes" セルマップを差し替える。
    player.set_cellmap_texture(None, "Clothes", &rgba, 1024, 1024)?;

    // …あとで元に戻す。
    player.restore_cellmap_texture(None, "Clothes");

    // 現在オーバーライドされているか？
    let swapped = player.is_cellmap_overridden(None, "Clothes");
    ```

    GPU テクスチャを自前で持つエンジンはピクセルコピーを省けます。セルマップの private key を取得し、自分の `TextureView` をそのキーで登録してください。

    ```rust
    use ssplayer_wgpu::AlphaMode;

    let key = player.override_cellmap_texture_key(None, "Clothes").unwrap();
    renderer.set_texture(&device, key, &view, (1024, 1024), AlphaMode::Straight);
    ```

=== "C"

    ```c
    /* rgba = width*height*4 の straight-alpha バイト列。 */
    ss_player_set_cellmap_texture(player, /* pack */ NULL, "Clothes", rgba, 1024, 1024);

    /* rgba に NULL を渡すと元に戻る（width/height は無視される）。 */
    ss_player_set_cellmap_texture(player, NULL, "Clothes", NULL, 0, 0);

    bool swapped = ss_player_is_cellmap_overridden(player, NULL, "Clothes");
    ```

=== "JavaScript"

    ```js
    // エンコード済みバイト列（PNG）から、ブラウザにデコードさせる：
    await player.setCellmapTextureFromBytes(null, 'Clothes', bytes);

    // …または、既にある premultiplied な ImageBitmap から
    // （createImageBitmap(blob, { premultiplyAlpha: 'premultiply' })）：
    player.setCellmapTexture(null, 'Clothes', imageBitmap);

    player.restoreCellmapTexture(null, 'Clothes');
    const swapped = player.isCellmapOverridden(null, 'Clothes');
    ```

=== "Bevy"

    ```rust
    use ssplayer_bevy::SsCellmapTextures;
    use bevy::image::ImageLoaderSettings;
    use bevy::platform::collections::HashMap;

    // 非 sRGB で読む。Player はアトラスのバイト列をそのまま扱いますが、Bevy は指定が
    // 無いと PNG を sRGB として読むため、衣装が暗く描かれます。
    let sailor = asset_server.load_with_settings("sailor.png", |s: &mut ImageLoaderSettings| s.is_srgb = false);

    // コンポーネントを挿入（または編集）する。まだロード中の画像は、準備できたフレームで
    // 適用されるので、直前にリクエストしたばかりのハンドルでも問題ありません。
    let mut overrides = HashMap::new();
    overrides.insert("Clothes".to_string(), sailor);
    commands.entity(character).insert(SsCellmapTextures { overrides });

    // エントリを（あるいはコンポーネントごと）削除すると元に戻ります。
    ```

差し替えは**キャラクター単位**の衣装です。同じパックを再生する別キャラクターは影響を受けず、インスタンスパーツのサブアニメ（自分のパックを再生する）にも伝搬しません — そちらにも効かせたいなら別途設定してください。

## 注意点

!!! note "`set_cellmap_texture` と `set_part_cell_override`"

    紛らわしい2種類の差し替えです。**`set_cellmap_texture`** は*セルマップのテクスチャ丸ごと*を差し替えます — そのセルマップを描く全パーツが一度に変わり、各セルの位置は保たれます。
    [**`set_part_cell_override`**](./part-overrides.md#cell) は*1つのパーツが描くセルそのもの*をパーツ単位で変えます。衣装の色替えは前者、片手だけ別の武器を持たせるのは後者です。

!!! warning "同じサイズ・同じレイアウト"

    ランタイムの UV は**元の**セルマップのピクセル空間のままで、プレーヤはバインドしたテクスチャのサイズで正規化します。したがって解像度が異なる差し替えは別の領域をサンプルしてしまうので、元と**同じサイズ・同じセルレイアウト**のアトラスに差し替えてください。

!!! tip "TextureChange の差し先を差し替える"

    オーサリングされた **TextureChange** の効いているパーツは、そのフレームだけ別のハッシュでバインドされます。セルマップ名を差し替えてもそのフレームには届かないので、TextureChange の差し先を差し替えたいなら、上の列挙に現れる **そのファイル名**（`…​.png` のエントリ）を指定してください。

Rust と C では差し替えは**次の update** で GPU に届きます — borrow model はそれまでプレーヤにデバイスを貸さないためで、差し替えは次フレームから見えます。別のアニメーションを選択しても維持されます。差し替えは現在のアニメーションツリーではなくキャラクターに紐づくためです。

## 次に読む

* [パーツオーバーライド](./part-overrides.md) — *1つのパーツ*が描くものを変える
* [ロードとアセット](./loading.md) — パック自身のアトラスがどこから来るか
