//! Regenerate `packs-index.toml` from the working tree. Repo tooling, not
//! part of the shipped binary — run via `scripts/gen-packs-index.sh`.
//! Optional argument: the release tag to pin (defaults to the tag already
//! in the committed index).

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let index_path = root.join("packs-index.toml");
    let tag = std::env::args().nth(1).unwrap_or_else(|| {
        btt::install::curated_index()
            .expect("committed packs-index.toml must parse")
            .tag
    });
    let (text, skipped) =
        btt::install::generate_index(root, &tag).expect("generating packs-index.toml");
    for reason in &skipped {
        eprintln!("skipped: {reason}");
    }
    std::fs::write(&index_path, &text).expect("writing packs-index.toml");
    println!("wrote {} (tag {tag})", index_path.display());
}
