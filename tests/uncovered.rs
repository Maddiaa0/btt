//! Tests for uncovered-file detection: source files that contain tests but
//! that no `.tree` spec routes to. Fixtures are written under
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

fn packs(name: &str) -> Vec<pack::Pack> {
    vec![pack::load(name, repo_root()).unwrap()]
}

const TESTED_RS: &str = "
mod tests {
    #[test]
    fn works() {}

    #[test]
    fn also_works() {}
}
";

const TESTED_TS: &str = r#"
describe("map", () => {
  it("works", () => {});
});
"#;

mod when_a_source_file_has_tests_but_no_tree {
    use super::*;

    #[test]
    fn is_reported_with_its_test_count() {
        let dir = fixture_dir("no-tree");
        std::fs::write(dir.join("map.rs"), TESTED_RS).unwrap();

        let scan = runner::find_uncovered(&packs("rust"), std::slice::from_ref(&dir), &[]);
        assert!(scan.failed.is_empty(), "{:?}", scan.failed);
        assert_eq!(scan.uncovered.len(), 1, "{:?}", scan.uncovered);
        assert_eq!(scan.uncovered[0].path, dir.join("map.rs"));
        assert_eq!(scan.uncovered[0].tests, 2);
    }
}

mod when_a_tree_spec_exists_next_to_the_file {
    use super::*;

    #[test]
    fn is_not_reported() {
        let dir = fixture_dir("has-tree");
        std::fs::write(dir.join("map.rs"), TESTED_RS).unwrap();
        std::fs::write(dir.join("map.tree"), "map\n└── it works\n").unwrap();

        let trees = vec![dir.join("map.tree")];
        let scan = runner::find_uncovered(&packs("rust"), std::slice::from_ref(&dir), &trees);
        assert!(scan.uncovered.is_empty(), "{:?}", scan.uncovered);
        assert!(scan.failed.is_empty(), "{:?}", scan.failed);
    }
}

mod when_a_source_file_has_no_tests {
    use super::*;

    #[test]
    fn is_not_reported() {
        let dir = fixture_dir("no-tests");
        std::fs::write(dir.join("lib.rs"), "pub fn helper() {}\n").unwrap();

        let scan = runner::find_uncovered(&packs("rust"), std::slice::from_ref(&dir), &[]);
        assert!(scan.uncovered.is_empty(), "{:?}", scan.uncovered);
        assert!(scan.failed.is_empty(), "{:?}", scan.failed);
    }
}

mod when_a_tree_routes_to_only_one_of_two_candidates {
    use super::*;

    // Coverage is routing-exact: the tree routes to map.test.ts (first
    // matching target pattern), so a test-bearing map.spec.ts must still
    // be reported — a same-stem `.tree` alone is not coverage.
    #[test]
    fn reports_the_unrouted_candidate() {
        let dir = fixture_dir("two-candidates");
        std::fs::write(dir.join("map.tree"), "map\n└── it works\n").unwrap();
        std::fs::write(dir.join("map.test.ts"), TESTED_TS).unwrap();
        std::fs::write(dir.join("map.spec.ts"), TESTED_TS).unwrap();

        let trees = vec![dir.join("map.tree")];
        let scan = runner::find_uncovered(&packs("typescript"), std::slice::from_ref(&dir), &trees);
        assert!(scan.failed.is_empty(), "{:?}", scan.failed);
        let paths: Vec<_> = scan.uncovered.iter().map(|u| u.path.clone()).collect();
        assert_eq!(paths, vec![dir.join("map.spec.ts")], "{:?}", scan.uncovered);
    }
}

mod when_a_candidate_cannot_be_scanned {
    use super::*;

    // Unverifiable coverage must surface as a failure, not vanish — a
    // strict project would otherwise pass because extraction broke.
    #[test]
    fn is_reported_as_failed() {
        let dir = fixture_dir("unscannable");
        std::fs::write(dir.join("map.test.ts"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();

        let scan = runner::find_uncovered(&packs("typescript"), std::slice::from_ref(&dir), &[]);
        assert!(scan.uncovered.is_empty(), "{:?}", scan.uncovered);
        assert_eq!(scan.failed.len(), 1, "{:?}", scan.failed);
        assert_eq!(scan.failed[0].0, dir.join("map.test.ts"));
    }
}
