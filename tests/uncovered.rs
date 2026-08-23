//! Tests for uncovered-file detection: source files that contain tests but
//! have no `.tree` spec routing to them. Fixtures are written under
//! `target/uncovered-fixtures/<case>/` so each case is isolated.

use btt::{pack, runner};
use std::path::{Path, PathBuf};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_dir(case: &str) -> PathBuf {
    let dir = repo_root().join("target/uncovered-fixtures").join(case);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn rust_packs() -> Vec<pack::Pack> {
    vec![pack::load("rust", repo_root()).unwrap()]
}

const TESTED_RS: &str = "
mod tests {
    #[test]
    fn works() {}

    #[test]
    fn also_works() {}
}
";

mod when_a_source_file_has_tests_but_no_tree {
    use super::*;

    #[test]
    fn is_reported_with_its_test_count() {
        let dir = fixture_dir("no-tree");
        std::fs::write(dir.join("map.rs"), TESTED_RS).unwrap();

        let found = runner::find_uncovered(&rust_packs(), std::slice::from_ref(&dir));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, dir.join("map.rs"));
        assert_eq!(found[0].tests, 2);
    }
}

mod when_a_tree_spec_exists_next_to_the_file {
    use super::*;

    #[test]
    fn is_not_reported() {
        let dir = fixture_dir("has-tree");
        std::fs::write(dir.join("map.rs"), TESTED_RS).unwrap();
        std::fs::write(dir.join("map.tree"), "map\n└── it works\n").unwrap();

        let found = runner::find_uncovered(&rust_packs(), &[dir]);
        assert!(found.is_empty(), "{found:?}");
    }
}

mod when_a_source_file_has_no_tests {
    use super::*;

    #[test]
    fn is_not_reported() {
        let dir = fixture_dir("no-tests");
        std::fs::write(dir.join("lib.rs"), "pub fn helper() {}\n").unwrap();

        let found = runner::find_uncovered(&rust_packs(), &[dir]);
        assert!(found.is_empty(), "{found:?}");
    }
}
