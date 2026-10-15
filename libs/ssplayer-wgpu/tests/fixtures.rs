//! The one test that fails when the fixtures are not there to test against.
//!
//! The suite reads `assets/`, which `scripts/deploy-examples.sh` converts from
//! the SDK's sample projects, and most tests return early when a pack does not
//! load — so without it, or after an SDK bump the converter's output no longer
//! parses under, the suite comes back green having verified nothing. This test
//! is what says so, as `gpu_adapter.rs` does for a missing adapter.
//!
//! It needs no GPU: `load` takes no device.

use ssplayer_wgpu::SSPlayer;

/// The directories the suite reads from. Each must hold at least one pack.
const DIRS: [&str; 3] = ["overall", "overall_emb", "Ringo"];

fn asset_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

#[test]
fn every_fixture_is_deployed_and_loads_whole() {
    let mut failures = Vec::new();
    for dir in DIRS {
        let path = format!("{}/{dir}", asset_root());
        let mut packs: Vec<_> = std::fs::read_dir(&path).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|p| p.extension().is_some_and(|x| x == "ssab")).collect();
        packs.sort();
        if packs.is_empty() {
            failures.push(format!("{path}: no .ssab"));
            continue;
        }
        for pack in packs {
            let mut player = SSPlayer::new();
            if let Err(err) = player.load(&pack) {
                failures.push(format!("{}: {err}", pack.display()));
                continue;
            }
            // `load` is Ok whenever the `.ssab` itself reads, so a pack whose
            // atlases or sibling packs went missing would still pass above.
            for missing in player.missing_assets() {
                failures.push(format!("{}: {:?} {} did not arrive", pack.display(), missing.kind, missing.name));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "the fixtures are missing or do not load, so every test that reads them returned early and verified nothing. \
         Run scripts/deploy-examples.sh.\n  {}",
        failures.join("\n  ")
    );
}
