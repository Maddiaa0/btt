# Releasing btt

Releases are cut from tags: pushing a `v*` tag to GitHub runs
[`.github/workflows/release.yml`](.github/workflows/release.yml), which

1. **verifies** the tag matches the `Cargo.toml` version, runs the tests, and
   dry-runs `cargo publish` (packaging sanity),
2. **builds** release binaries for Linux (x86_64 gnu/musl, aarch64 gnu),
   macOS (x86_64, aarch64), and Windows (x86_64),
3. **creates the GitHub release** with the archives, a `SHA256SUMS` file, and
   auto-generated notes from merged PRs,
4. **publishes to crates.io** — last, because it is the only irreversible
   step (a bad GitHub release can be deleted; a crates.io version can only be
   yanked).

## Cutting a release

```bash
scripts/release.sh 0.2.0          # bump, verify, commit, tag
git push origin master v0.2.0     # or pass --push to the script
```

The script requires a clean tree on an up-to-date `master`. It bumps the
version in `Cargo.toml`, refreshes only btt's entry in `Cargo.lock`
(`cargo update --workspace` — it never re-resolves dependencies, so the
14-day min-publish-age policy is unaffected), runs the full local check
suite (fmt, tests, clippy, `btt check` self-check, `cargo publish
--dry-run`), then commits `chore: release vX.Y.Z` and creates the annotated
tag.

If direct pushes to `master` are blocked, run the script on a branch after
editing the branch check out — or simpler: open a PR with just the version
bump, and once it merges, tag the merge commit:

```bash
git tag -a v0.2.0 -m "btt 0.2.0" && git push origin v0.2.0
```

The workflow's tag/version check makes tagging the wrong commit fail fast.

## One-time setup

- `CARGO_REGISTRY_TOKEN` repository secret: a crates.io API token
  (crates.io → Account Settings → API Tokens) scoped to `publish-new` +
  `publish-update` for the `btt` crate.

## If something fails mid-release

Jobs run in order, so a failure never leaves a half-published crate:

- **verify/build failed**: nothing was published. Fix on `master`, delete the
  tag (`git push origin :refs/tags/v0.2.0 && git tag -d v0.2.0`), and re-cut.
- **crates.io publish failed** (e.g. missing token): the GitHub release
  already exists and is fine. Fix the cause and re-run the failed job from
  the Actions UI.
