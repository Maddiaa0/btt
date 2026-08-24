#!/usr/bin/env bash
# Cut a release: bump the version, verify, commit, and tag.
#
#   scripts/release.sh 0.2.0          # bump + verify + commit + tag, print push command
#   scripts/release.sh 0.2.0 --push   # also push master and the tag
#
# Pushing the tag triggers .github/workflows/release.yml, which re-verifies,
# builds binaries, creates the GitHub release, and publishes to crates.io.
set -euo pipefail
cd "$(dirname "$0")/.."

die() { echo "error: $*" >&2; exit 1; }

version="${1:-}"
[[ -n "$version" ]] || die "usage: scripts/release.sh <version> [--push]"
push=false
[[ "${2:-}" == "--push" ]] && push=true
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] \
  || die "'$version' is not a semver version (e.g. 0.2.0)"
tag="v$version"

# Preflight: clean tree, on master, in sync with origin, tag unused.
[[ -z "$(git status --porcelain)" ]] || die "working tree is not clean"
branch="$(git rev-parse --abbrev-ref HEAD)"
[[ "$branch" == "master" ]] || die "release from master (currently on $branch)"
git fetch origin master --tags
[[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/master)" ]] \
  || die "master is not in sync with origin/master"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && die "tag $tag already exists"

current="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')"
[[ "$version" != "$current" ]] || die "version is already $current"

echo "==> bumping $current -> $version"
sed -i.bak "0,/^version = \"$current\"/s//version = \"$version\"/" Cargo.toml
rm Cargo.toml.bak
# Refresh only btt's own entry in Cargo.lock. --workspace never re-resolves
# dependencies, so the 14-day min-publish-age policy is not in play here.
cargo update --workspace --quiet

echo "==> verifying (fmt, test, clippy, self-check, package)"
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo run --locked --quiet -- check
cargo publish --dry-run --locked --allow-dirty

echo "==> committing and tagging $tag"
git commit -am "chore: release $tag"
git tag -a "$tag" -m "btt $version"

if $push; then
  git push origin master "$tag"
  echo "==> pushed; release workflow is running: https://github.com/Maddiaa0/btt/actions"
else
  echo "==> done. To publish the release, run:"
  echo "    git push origin master $tag"
fi
