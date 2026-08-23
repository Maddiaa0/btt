//! btt checks its own repository: every `.tree` file under src/ and tests/
//! must match its test file. This is the same pipeline `btt check` runs.

use btt::config::CheckConfig;
use btt::pack;
use btt::runner::{self, Target};
use std::path::Path;

mod when_checking_btt_against_its_own_trees {
    use super::*;

    #[test]
    fn finds_no_drift() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let packs = vec![pack::load("rust", root).unwrap()];
        let tree_files = runner::find_tree_files(&[root.join("src"), root.join("tests")]).unwrap();
        assert!(!tree_files.is_empty(), "no .tree files found in repo");

        let mut problems = Vec::new();
        for tree_path in &tree_files {
            match runner::resolve_target(tree_path, &packs) {
                Target::Found { pack, path } => {
                    let reported =
                        runner::check_file(pack, tree_path, &path, &CheckConfig::default())
                            .unwrap();
                    for r in reported {
                        problems.push(format!("{}: {:?}", tree_path.display(), r.finding));
                    }
                }
                Target::NotFound { .. } => {
                    problems.push(format!("{}: no matching test file", tree_path.display()));
                }
            }
        }
        assert!(
            problems.is_empty(),
            "BTT drift in this repo:\n{}",
            problems.join("\n")
        );
    }
}
