//! Tests for the pack trust boundary: manifests are strictly parsed and
//! every pack-controlled path is confined to the pack directory. Fixture
//! packs are written under `target/hardening-fixtures/<case>/`.

use btt::error::Error;
use btt::pack;
use std::path::{Path, PathBuf};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const VALID_MANIFEST: &str = r#"
[pack]
name = "fixture"
version = "0.0.1"

[detect]
targets = ["{stem}.rs"]

[grammar]
source = "builtin:rust"

[extract]
query = "queries/tests.scm"

[scaffold]
template = "templates/test.jinja"
output = "{stem}.rs"
"#;

/// Write a loadable pack, then apply `mutate` to the manifest text.
fn fixture_pack(case: &str, mutate: impl Fn(&str) -> String) -> PathBuf {
    let dir = repo_root().join("target/hardening-fixtures").join(case);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("queries")).unwrap();
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::write(dir.join("queries/tests.scm"), "").unwrap();
    std::fs::write(dir.join("templates/test.jinja"), "").unwrap();
    std::fs::write(dir.join("pack.toml"), mutate(VALID_MANIFEST)).unwrap();
    dir
}

mod when_a_pack_manifest_references_paths_outside_its_directory {
    use super::*;

    #[test]
    fn refuses_to_load() {
        for (field, escaped) in [
            ("queries/tests.scm", "../../outside.scm"),
            ("templates/test.jinja", "/etc/hostname"),
            ("output = \"{stem}.rs\"", "output = \"../{stem}.rs\""),
        ] {
            let dir = fixture_pack("path-escape", |m| m.replace(field, escaped));
            let err = pack::load_dir(&dir).unwrap_err();
            assert!(
                matches!(err, Error::UnsafePath { .. }),
                "{field} -> {escaped}: {err}"
            );
        }
    }
}

mod when_a_pack_file_is_a_symlink_escaping_the_pack {
    use super::*;

    // confine() checks the textual path; symlinks are the way around it,
    // so reads resolve the real path and require it to stay inside.
    #[test]
    fn refuses_to_load() {
        let dir = fixture_pack("symlink-escape", ToString::to_string);
        let outside = repo_root().join("target/hardening-fixtures/outside.jinja");
        std::fs::write(&outside, "leaked").unwrap();
        std::fs::remove_file(dir.join("templates/test.jinja")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("templates/test.jinja")).unwrap();

        let err = pack::load_dir(&dir).unwrap_err();
        assert!(matches!(err, Error::UnsafePath { .. }), "{err}");
    }
}

mod when_a_pack_name_is_not_a_single_path_component {
    use super::*;

    #[test]
    fn refuses_to_load() {
        for name in ["../evil", "a/b", "/abs", ".."] {
            let err = pack::load(name, repo_root()).unwrap_err();
            assert!(matches!(err, Error::UnsafePath { .. }), "{name}: {err}");
        }
    }
}

mod when_a_manifest_contains_unknown_fields {
    use super::*;

    #[test]
    fn refuses_to_load() {
        // A typo of test_requires_marker: silently ignoring it would turn
        // marker enforcement off without any signal to the pack author.
        let dir = fixture_pack("unknown-field", |m| {
            m.replace(
                "query = \"queries/tests.scm\"",
                "query = \"queries/tests.scm\"\ntest_require_marker = true",
            )
        });
        let err = pack::load_dir(&dir).unwrap_err();
        assert!(matches!(err, Error::Toml { .. }), "{err}");
        assert!(err.to_string().contains("test_require_marker"), "{err}");
    }
}
