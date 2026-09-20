//! Filling the preview shell: a pack, everything it depends on, and two holes.
//!
//! The shell (`examples/preview/shell.html`, built by `scripts/build-preview.sh`)
//! is a complete page with nothing in it. This is the other half — the step a
//! host's "Preview" button runs: gather what the pack needs, base64 it, and put
//! it in the holes.
//!
//! **The gathering is the part worth having.** Substituting two strings is
//! nothing; knowing *which* files to substitute means reading the `.ssab`, and
//! getting it wrong is silent — a missing atlas draws white silhouettes and a
//! missing sibling pack draws nothing at all where an instance part should be.
//! [`SSPack::textures`] and [`SSPack::instance_packs`] are the two read-outs
//! that answer it, and this walk is what a host in another language ports.
//!
//! It is deliberately **not** in the player: the player has no filesystem on
//! every target it builds for, and a tool that writes HTML has no business in a
//! renderer.

use ssplayer_wgpu::SSPack;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The hole that holds the pack itself, as it appears in an unfilled shell.
const PACK_HOLE: &str = r#"<script id="ssab" type="application/base64"></script>"#;
/// The hole that holds everything else, keyed by file name.
const FILES_HOLE: &str = r#"<script id="files" type="application/json">{}</script>"#;

/// A file the page could not be given, and why it matters.
#[derive(Debug, PartialEq, Eq)]
pub struct Missing {
    /// What was looked for, as the `.ssab` named it.
    pub path: String,
    /// What its absence costs on screen.
    pub effect: &'static str,
}

/// What [`fill`] gathered, beside the page itself.
#[derive(Debug, Default)]
pub struct Report {
    /// Sibling packs inlined, by the name instance parts resolve against.
    pub packs: Vec<String>,
    /// Atlases inlined, by the path the pack named.
    pub atlases: Vec<String>,
    /// Everything that could not be found. A preview still opens without them.
    pub missing: Vec<Missing>,
}

/// Fill `shell` with `pack` and everything it depends on.
///
/// `animation` is what the page should select on open; `None` leaves it on the
/// pack's first. A file that cannot be found is reported rather than fatal —
/// a preview of a half-present pack is still worth opening, and it says on
/// screen what is absent.
pub fn fill(shell: &str, pack: &Path, animation: Option<&str>) -> Result<(String, Report), String> {
    // Refuse a shell that has already been filled, or one from a build that no
    // longer matches this tool. Substituting nothing and writing the file anyway
    // produces a preview that opens to "drop a .ssab here" and blames the user.
    for hole in [PACK_HOLE, FILES_HOLE] {
        if !shell.contains(hole) {
            return Err(format!("the shell has no {hole} — it is already filled, or it was built by a different version"));
        }
    }

    let root = pack.parent().unwrap_or(Path::new(""));
    let bytes = std::fs::read(pack).map_err(|error| format!("{}: {error}", pack.display()))?;
    let main = SSPack::load(&bytes).map_err(|error| format!("{} is not a readable .ssab: {error:?}", pack.display()))?;

    let mut report = Report::default();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    // Breadth-first over instance references, because a sibling pack has
    // siblings of its own. `<the .ssab's directory>/<name>.ssab` is the rule the
    // player's own loading walk follows, and the one the exporter writes for.
    let mut queue: Vec<(SSPack, PathBuf)> = vec![(main.clone(), pack.to_path_buf())];
    let mut seen: Vec<String> = vec![main.name().to_string()];
    while let Some((current, at)) = queue.pop() {
        let here = at.parent().unwrap_or(root).to_path_buf();

        for texture in current.textures() {
            if texture.embedded || files.contains_key(&texture.path) {
                continue;
            }
            match std::fs::read(here.join(&texture.path)) {
                Ok(image) => {
                    files.insert(texture.path.clone(), image);
                    report.atlases.push(texture.path.clone());
                }
                Err(_) => report.missing.push(Missing { path: texture.path.clone(), effect: "the parts using it draw as white silhouettes" }),
            }
        }

        for name in current.instance_packs() {
            if seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            let sibling = here.join(format!("{name}.ssab"));
            let Ok(data) = std::fs::read(&sibling) else {
                report.missing.push(Missing { path: format!("{name}.ssab"), effect: "the instance parts playing it draw nothing" });
                continue;
            };
            match SSPack::load(&data) {
                Ok(loaded) => {
                    files.insert(format!("{name}.ssab"), data);
                    report.packs.push(name.clone());
                    queue.push((loaded, sibling));
                }
                Err(_) => report.missing.push(Missing { path: format!("{name}.ssab"), effect: "it is not a readable .ssab" }),
            }
        }
    }
    report.atlases.sort();
    report.packs.sort();

    let name = pack.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut pack_tag = format!(r#"<script id="ssab" type="application/base64" data-name="{}""#, escape(&name));
    if let Some(animation) = animation {
        pack_tag.push_str(&format!(r#" data-animation="{}""#, escape(animation)));
    }
    pack_tag.push('>');
    pack_tag.push_str(&base64(&bytes));
    pack_tag.push_str("</script>");

    let mut json = String::from("{");
    for (index, (key, data)) in files.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        // The keys are file names out of the `.ssab`, and JSON is the only thing
        // reading them back, so a quote or a backslash in a path is the whole
        // escaping problem.
        json.push_str(&format!("\"{}\":\"{}\"", key.replace('\\', "\\\\").replace('"', "\\\""), base64(data)));
    }
    json.push('}');

    let page = shell.replace(PACK_HOLE, &pack_tag).replace(FILES_HOLE, &format!(r#"<script id="files" type="application/json">{json}</script>"#));
    Ok((page, report))
}

/// Minimal HTML attribute escaping. The values are a file name and an animation
/// name, both authored, so this is about a stray quote rather than about
/// hostile input — but an unescaped one silently truncates the attribute.
fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Standard base64, no line breaks. Hand-rolled rather than depending on a
/// crate: it is twelve lines, and the alternative is a dependency in the tree
/// of something that ships nothing.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bits = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(ALPHABET[(bits >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(bits >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(bits >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[(bits & 63) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // Every bit pattern, so a wrong shift shows up rather than hiding in
        // the printable range.
        let all: Vec<u8> = (0..=255u8).collect();
        assert_eq!(base64(&all).len(), 344);
        assert!(base64(&all).starts_with("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4v"));
    }

    #[test]
    fn a_shell_without_the_holes_is_refused() {
        let error = fill("<html></html>", Path::new("nope.ssab"), None).unwrap_err();
        assert!(error.contains("has no"), "{error}");
    }
}
