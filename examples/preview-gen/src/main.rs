//! Fill the preview shell from the command line.
//!
//! ```sh
//! cargo run -p ssplayer-preview-gen -- pack=assets/Ringo/Ringo.ssab open=yes
//! ```
//!
//! `key=value` arguments, the same shape the repository's shell scripts take.
//! The work is in [`ssplayer_preview_gen::fill`]; this is argument parsing, one
//! file read, one file write and an optional launch.

use ssplayer_preview_gen::fill;
use std::path::{Path, PathBuf};

fn usage() -> ! {
    eprintln!("Usage: ssplayer-preview-gen pack=<file.ssab> [options]");
    eprintln!("  pack=<file.ssab>     The pack to preview. Its atlases and sibling packs");
    eprintln!("                       are found beside it and inlined.");
    eprintln!("  shell=<file.html>    The shell to fill (default: bin/ssplayer-preview.html,");
    eprintln!("                       built by scripts/build-preview.sh)");
    eprintln!("  out=<file.html>      Where to write it (default: <pack stem>-preview.html here)");
    eprintln!("  animation=<name>     Which animation to open on (default: the pack's first)");
    eprintln!("  open=yes             Open the result in the default browser afterwards");
    std::process::exit(1)
}

fn main() {
    let (mut pack, mut shell, mut out, mut animation, mut open) = (None, PathBuf::from("bin/ssplayer-preview.html"), None, None, false);
    for argument in std::env::args().skip(1) {
        match argument.split_once('=') {
            Some(("pack", value)) => pack = Some(PathBuf::from(value)),
            Some(("shell", value)) => shell = PathBuf::from(value),
            Some(("out", value)) => out = Some(PathBuf::from(value)),
            Some(("animation", value)) => animation = Some(value.to_string()),
            Some(("open", value)) => open = value == "yes",
            _ => usage(),
        }
    }
    let Some(pack) = pack else { usage() };

    let source = match std::fs::read_to_string(&shell) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("cannot read the shell {}: {error}", shell.display());
            eprintln!("build one first: ./scripts/build-preview.sh");
            std::process::exit(1);
        }
    };

    let (page, report) = match fill(&source, &pack, animation.as_deref()) {
        Ok(filled) => filled,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };

    let out = out.unwrap_or_else(|| PathBuf::from(format!("{}-preview.html", pack.file_stem().unwrap_or_default().to_string_lossy())));
    if let Err(error) = std::fs::write(&out, &page) {
        eprintln!("cannot write {}: {error}", out.display());
        std::process::exit(1);
    }

    println!("{} ({} KiB)", out.display(), page.len() / 1024);
    if !report.packs.is_empty() {
        println!("  packs:   {}", report.packs.join(", "));
    }
    if !report.atlases.is_empty() {
        println!("  atlases: {}", report.atlases.join(", "));
    }
    // Loud, because the alternative is a preview that quietly draws the wrong
    // thing and gets reported as a renderer bug.
    for missing in &report.missing {
        eprintln!("  missing: {} — {}", missing.path, missing.effect);
    }

    if open {
        launch(&out);
    }
}

/// Hand the file to the desktop. `file://` is all the page needs, which is the
/// whole reason this tool ends here rather than at a server.
fn launch(path: &Path) {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (program, first) = if cfg!(target_os = "macos") {
        ("open", None)
    } else if cfg!(target_os = "windows") {
        ("cmd", Some("/C"))
    } else {
        ("xdg-open", None)
    };
    let mut command = std::process::Command::new(program);
    if let Some(first) = first {
        command.arg(first).arg("start").arg("");
    }
    if let Err(error) = command.arg(&absolute).spawn() {
        eprintln!("could not open it ({error}); the file is at {}", absolute.display());
    }
}
