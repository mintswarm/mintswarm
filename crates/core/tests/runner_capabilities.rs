//! The runner reacts to nothing, reads no collaborators and never merges: no source in the
//! runner crate implements [`Reactions`], [`Collaborators`] or [`Merges`] for any type, its
//! tracker included (design §13 P4). It may implement [`CommentEdits`], under the cross-App
//! fallback.
//!
//! [`Reactions`]: mintswarm_domain::tracker::Reactions
//! [`Collaborators`]: mintswarm_domain::tracker::Collaborators
//! [`Merges`]: mintswarm_domain::tracker::Merges
//! [`CommentEdits`]: mintswarm_domain::tracker::CommentEdits
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use std::path::{Path, PathBuf};

/// Built from two halves so that this file does not contain them.
const NEEDLES: [&str; 3] = [
    concat!("Reactions", " for"),
    concat!("Collaborators", " for"),
    concat!("Merges", " for"),
];

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn the_runner_implements_none_of_reactions_collaborators_or_merges() {
    let runner = Path::new(env!("CARGO_MANIFEST_DIR")).join("../runner");
    let mut paths = vec![runner.join("Cargo.toml")];
    files(&runner.join("src"), &mut paths);
    files(&runner.join("tests"), &mut paths);
    assert!(paths.iter().any(|p| p.ends_with("src/lib.rs")), "{paths:?}");
    for path in &paths {
        let text = std::fs::read(path).unwrap();
        for needle in NEEDLES.map(str::as_bytes) {
            assert!(
                !text.windows(needle.len()).any(|window| window == needle),
                "{} implements {}",
                path.display(),
                String::from_utf8_lossy(needle)
            );
        }
    }
}
