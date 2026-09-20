//! What the generator gathers, against the packs the repository already ships.
//!
//! The substitution is trivial and the gathering is not, so these run over real
//! `.ssab` files: a reference pack that needs its atlases beside it, a pack
//! whose instance parts play from a sibling, and an embedded pack that needs
//! nothing at all. A fake shell stands in for the built one — it is 1.3 MB of
//! wasm that says nothing about whether the right files were found, and
//! building it needs wasm-pack.

use ssplayer_preview_gen::fill;
use std::path::{Path, PathBuf};

/// Only the two holes, which is all `fill` looks for.
const SHELL: &str = concat!("<html><body>\n", r#"<script id="ssab" type="application/base64"></script>"#, "\n", r#"<script id="files" type="application/json">{}</script>"#, "\n</body></html>",);

fn asset(path: &str) -> PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")).join(path)
}

/// A reference pack brings its atlases, and nothing else.
#[test]
fn a_reference_pack_inlines_the_images_it_names() {
    let pack = asset("overall/Basic.ssab");
    if !pack.exists() {
        eprintln!("assets are not deployed — skipping");
        return;
    }
    let (page, report) = fill(SHELL, &pack, None).unwrap();

    assert!(!report.atlases.is_empty(), "a reference pack must name at least one atlas");
    assert!(report.missing.is_empty(), "everything it names sits beside it: {:?}", report.missing);
    for atlas in &report.atlases {
        assert!(page.contains(&format!("\"{atlas}\":\"")), "{atlas} is named but was not inlined");
    }
    // No hole survives, and the page says which file it came from.
    assert!(!page.contains(r#"<script id="ssab" type="application/base64"></script>"#));
    assert!(!page.contains(r#"<script id="files" type="application/json">{}</script>"#));
    assert!(page.contains(r#"data-name="Basic.ssab""#));
}

/// The one a page cannot recover from on its own: an instance part plays from
/// another `.ssab`, and without it that part draws nothing at all.
#[test]
fn a_pack_with_instance_parts_inlines_the_pack_they_play_from() {
    let pack = asset("overall/Instance.ssab");
    if !pack.exists() {
        eprintln!("assets are not deployed — skipping");
        return;
    }
    let (page, report) = fill(SHELL, &pack, None).unwrap();

    assert!(!report.packs.is_empty(), "Instance.ssab plays from a sibling pack; none was gathered");
    for name in &report.packs {
        assert!(page.contains(&format!("\"{name}.ssab\":\"")), "{name} was gathered but not inlined");
    }
    assert!(report.missing.is_empty(), "{:?}", report.missing);
}

/// An embedded pack is one file. The second hole stays empty, and that is
/// correct rather than a failure to find anything.
#[test]
fn an_embedded_pack_needs_nothing_beside_it() {
    let pack = asset("overall_emb/Basic.ssab");
    if !pack.exists() {
        eprintln!("assets are not deployed — skipping");
        return;
    }
    let (page, report) = fill(SHELL, &pack, None).unwrap();

    assert!(report.atlases.is_empty(), "an embedded pack carries its own: {:?}", report.atlases);
    assert!(report.missing.is_empty(), "{:?}", report.missing);
    assert!(page.contains(r#"<script id="files" type="application/json">{}</script>"#), "the files hole should be filled with an empty object");
}

/// A pack on its own, away from its atlases: reported, and still previewable.
/// Silence here is what turns "my atlas was not exported" into "the renderer
/// draws white boxes".
#[test]
fn a_pack_without_its_atlases_says_so_and_still_produces_a_page() {
    let pack = asset("overall/Basic.ssab");
    if !pack.exists() {
        eprintln!("assets are not deployed — skipping");
        return;
    }
    let alone = std::env::temp_dir().join("ssplayer-preview-gen-alone");
    std::fs::create_dir_all(&alone).unwrap();
    let copied = alone.join("Basic.ssab");
    std::fs::copy(&pack, &copied).unwrap();

    let (page, report) = fill(SHELL, &copied, None).unwrap();
    assert!(!report.missing.is_empty(), "the atlases are not there; that must be reported");
    assert!(report.missing.iter().all(|missing| !missing.effect.is_empty()), "a report has to say what the absence costs");
    assert!(page.contains("data-name=\"Basic.ssab\""), "and the page is still written");

    std::fs::remove_dir_all(&alone).ok();
}

/// The animation a host had selected rides along, so the page opens on it
/// rather than on whatever the pack happens to list first.
#[test]
fn the_requested_animation_reaches_the_page() {
    let pack = asset("overall/Basic.ssab");
    if !pack.exists() {
        eprintln!("assets are not deployed — skipping");
        return;
    }
    let (page, _) = fill(SHELL, &pack, Some("anime_1")).unwrap();
    assert!(page.contains(r#"data-animation="anime_1""#));

    // And an attribute-breaking name is escaped rather than truncating the tag.
    let (page, _) = fill(SHELL, &pack, Some(r#"a"b"#)).unwrap();
    assert!(page.contains(r#"data-animation="a&quot;b""#), "the quote was not escaped");
}
