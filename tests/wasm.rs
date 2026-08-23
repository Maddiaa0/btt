//! Equivalence tests for sandboxed WASM grammars: the packs-wasm/ variants
//! of the builtin packs must extract exactly the same structure as their
//! natively compiled twins (same grammar versions, so byte-for-byte parity
//! is expected).
//!
//! Requires `--features wasm` and `scripts/fetch-wasm-grammars.sh`.
#![cfg(feature = "wasm")]

use btt::extract::{self, ActualNode};
use btt::pack::{self, Pack};
use std::path::Path;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn wasm_pack(name: &str) -> Pack {
    let dir = repo_root().join("packs-wasm").join(name);
    assert!(
        dir.join("grammar.wasm").is_file(),
        "missing {}/grammar.wasm — run scripts/fetch-wasm-grammars.sh",
        dir.display()
    );
    pack::load_dir(&dir).unwrap()
}

fn extract_both(
    builtin: &str,
    wasm: &str,
    target: &str,
    source: &str,
) -> (Vec<ActualNode>, Vec<ActualNode>) {
    let native_pack = pack::load(builtin, repo_root()).unwrap();
    let wasm_pack = wasm_pack(wasm);
    let native = extract::extract(&native_pack, Path::new(target), source).unwrap();
    let sandboxed = extract::extract(&wasm_pack, Path::new(target), source).unwrap();
    (native, sandboxed)
}

const RUST_SOURCE: &str = r"
pub fn lookup() {}

#[cfg(test)]
mod tests {
    fn helper() {}

    mod when_the_key_is_present {
        #[test]
        fn returns_the_value() {}

        mod when_the_value_was_overwritten {
            #[tokio::test]
            async fn returns_the_latest_value() {}
        }
    }

    mod when_the_key_is_absent {
        #[test]
        fn returns_none() {}
    }
}
";

const TS_SOURCE: &str = r#"
import { describe, it } from "vitest";
const helper = () => {};
describe("HashMap", () => {
  describe("when the key is present", () => {
    it("returns the value", () => {});
  });
  describe("when the key is absent", () => {
    it.skip("returns none", () => {});
  });
});
"#;

mod when_parsing_rust_through_a_wasm_grammar {
    use super::*;

    #[test]
    fn matches_the_native_extraction() {
        let (native, sandboxed) = extract_both("rust", "rust", "map.rs", RUST_SOURCE);
        assert!(!native.is_empty());
        assert_eq!(native, sandboxed);
    }
}

mod when_parsing_typescript_through_a_wasm_grammar {
    use super::*;

    #[test]
    fn matches_the_native_extraction() {
        let (native, sandboxed) =
            extract_both("typescript", "typescript", "map.test.ts", TS_SOURCE);
        assert!(!native.is_empty());
        assert_eq!(native, sandboxed);
    }
}

mod when_a_wasm_grammar_fails_to_load {
    use super::*;

    // Regression: a failed load used to drop the thread's wasm store, so
    // every grammar already cached on that thread stopped working. The
    // whole sequence must run in one #[test] fn (= one thread).
    #[test]
    fn does_not_poison_other_grammars_on_the_thread() {
        let good = wasm_pack("typescript");
        let first = extract::extract(&good, Path::new("map.test.ts"), TS_SOURCE).unwrap();
        assert!(!first.is_empty());

        let mut bad = wasm_pack("rust");
        bad.wasm_grammar = Some(vec![0x00, 0x61, 0x73, 0x6d]); // truncated module
        extract::extract(&bad, Path::new("map.rs"), RUST_SOURCE).unwrap_err();

        let again = extract::extract(&good, Path::new("map.test.ts"), TS_SOURCE).unwrap();
        assert_eq!(first, again);
    }
}

mod when_two_packs_share_a_grammar_symbol {
    use super::*;

    // The check is a deterministic pre-flight over the whole pack set —
    // never a per-thread cache probe, whose outcome would depend on which
    // rayon worker touched which pack first (same project could pass under
    // -j2 and fail under -j1).
    #[test]
    fn reports_a_collision_instead_of_reusing_the_wrong_grammar() {
        let ts = wasm_pack("typescript");
        let mut imposter = wasm_pack("rust");
        imposter.manifest.grammar.symbol = Some("typescript".to_string());
        let err = pack::validate_set(&[ts, imposter]).unwrap_err();
        assert!(err.to_string().contains("distinct symbol"), "{err}");

        // Same symbol, same bytes is not a collision.
        pack::validate_set(&[wasm_pack("typescript"), wasm_pack("typescript")]).unwrap();
    }
}
