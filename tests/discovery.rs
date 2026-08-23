//! Discovery must fail closed: a search path that cannot be walked is a
//! tool error, never an empty (silently passing) result.

use btt::pack;
use btt::runner;
use std::path::{Path, PathBuf};

mod when_a_search_path_does_not_exist {
    use super::*;

    #[test]
    fn fails_tree_discovery_as_a_tool_error() {
        let missing = vec![PathBuf::from("/definitely/not/a/real/path")];
        runner::find_tree_files(&missing).unwrap_err();
    }

    #[test]
    fn is_reported_as_failed_by_the_uncovered_scan() {
        let missing = vec![PathBuf::from("/definitely/not/a/real/path")];
        let packs = vec![pack::load("rust", Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap()];
        let scan = runner::find_uncovered(&packs, &missing, &[]);
        assert!(!scan.failed.is_empty());
    }
}
