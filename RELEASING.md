# Releasing btt

Releases are built by `dist` from version tags. The generated workflow builds
the `btt` executable for macOS, Linux, and Windows; creates shell, PowerShell,
npm, Homebrew, and MSI installers; publishes checksums and GitHub attestations;
and then publishes the package-manager entries.

## One-time publisher setup

Complete these steps before pushing the first release tag:

1. Sign in to crates.io, create an API token allowed to publish a new crate,
   and add it to this repository as the Actions secret
   `CARGO_REGISTRY_TOKEN`. The registry package is `btt-cli`; it installs the
   `btt` library and executable.
2. Create the public repository `Maddiaa0/homebrew-tap`, initialize it with a
   README, and add a token that can write that repository as the Actions secret
   `HOMEBREW_TAP_TOKEN` here.
3. Ensure the npm scope `@maddiaa0` exists, create a granular token allowed to
   publish a new package in that scope, and add it as the Actions secret
   `NPM_TOKEN` here. The generated package name is `@maddiaa0/btt`.

GitHub Releases and build-provenance attestations use the workflow's built-in
`GITHUB_TOKEN`; they need no additional secret.

## Cut a release

1. Update the version in `Cargo.toml` and update release notes.
2. Fetch the pinned WASM fixtures and run the complete verification suite:

   ```console
   $ ./scripts/fetch-wasm-grammars.sh
   $ cargo run --locked -- check
   $ cargo test --locked
   $ cargo clippy --locked --all-targets -- -D warnings
   $ cargo test --locked --features wasm
   $ cargo clippy --locked --all-targets --features wasm -- -D warnings
   $ cargo publish --locked --dry-run
   $ dist plan
   ```

3. Commit the version, merge it to the default branch, and tag that exact
   commit. The tag must contain the same semantic version as `Cargo.toml`:

   ```console
   $ git tag v0.2.0
   $ git push origin v0.2.0
   ```

The tag starts `.github/workflows/release.yml`. Do not publish the Cargo, npm,
or Homebrew packages separately: the release workflow publishes them only
after their referenced GitHub artifacts are available.

`dist` owns `.github/workflows/release.yml` and `wix/main.wxs`. Change release
settings in `dist-workspace.toml`, then rerun `dist init` or `dist generate`
instead of editing those generated files directly.
