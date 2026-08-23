# Agent guide for btt

## Testing is tree-first (dogfooded)

Tests in this repo follow the Branching Tree Technique this tool implements.
Every test module has a `.tree` spec next to it (`src/*.tree`,
`tests/*.tree`). Workflow: update the `.tree` first, then the tests, then
verify with `cargo run -- check` (also enforced by `tests/selfcheck.rs`).
Never add, rename, or reorder tests without updating the spec tree.

Rust test-file shape follows location (what `btt scaffold` emits):
`src/*.rs` wrap tests in `#[cfg(test)] mod tests { use super::*; … }`
with the `mod when_*` branches inside; `tests/*.rs` are integration
crates and stay flat — `mod when_*` at the file root, no wrapper. The
checker treats the `tests` wrapper as transparent, so `.tree` specs are
identical either way.

Before finishing any change: `cargo test` and
`cargo clippy --all-targets -- -D warnings` (pedantic is enforced via
`[lints]`). If you touched wasm-grammar code, also run both with
`--features wasm` (grammars come from `scripts/fetch-wasm-grammars.sh`).

## Dependency policy: no crates younger than 14 days

Freshly published crate versions are the main supply-chain attack window.
Never resolve to a crates.io version published less than 14 days ago,
including transitive dependencies.

This is enforced natively by cargo's `min-publish-age` (unstable): the
global `~/.cargo/config.toml` sets `[unstable] min-publish-age = true` and
`[registry] min-publish-age = "14 days"`, so **resolve dependency changes
with nightly cargo** and the policy applies automatically:

```bash
cargo +nightly add <crate>
cargo +nightly generate-lockfile   # re-resolve the whole lockfile
```

Rules:

- Never run bare `cargo update` (stable cargo has no age filter and pulls
  the freshest versions). On stable, downgrade single crates with
  `cargo update -p <crate> --precise <version>`.
- A conscious exception requires asking the user first; the escape hatch is
  `CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE=allow`.
- WASM grammar artifacts are pinned by release tag + sha256 in
  `scripts/fetch-wasm-grammars.sh`; update both together.
