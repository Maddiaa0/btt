# Pack releases

Each language pack is an independently versioned product. The repository is
a monorepo for authoring and testing them; it is not one versioned "pack
bundle."

## The three compatibility axes

Every `pack.toml` declares:

```toml
format = 1

[pack]
name = "example"
version = "0.2.0"

[compat]
btt = ">=0.2.0"
```

- `pack.version` is the SemVer version of that pack's observable behavior.
- `format` is the integer version of the manifest schema. Compatible fields
  do not change it; an incompatible schema becomes format 2.
- `compat.btt` is a SemVer requirement for the runtime features the pack uses.

The loader parses both SemVer fields, rejects unsupported formats before
interpreting their bodies, and refuses a pack that does not support the
running `btt`. Unknown fields within a supported format remain errors.

## What constitutes a breaking pack change

Treat compatibility as an outcome, not as a count of edited files. A change
is breaking when an existing project can start failing or can scaffold a
materially different test suite. This includes changes to:

- target detection or priority;
- extraction and which constructs count as tests or blocks;
- mapping, normalization, or wrappers;
- the scaffold's output path or generated structure; and
- the minimum supported `btt` version.

Before 1.0, increment the minor for a breaking change (`0.2` to `0.3`) and
the patch for a compatible fix. After 1.0, use the usual major/minor/patch
SemVer meanings. If a fix intentionally corrects previously missed tests,
assume it can fail strict CI and classify it as breaking unless the affected
behavior was explicitly documented as a defect.

Native and WASM twins are two distributions of the same behavior. Their
queries, templates, pack versions, manifest formats, and `btt` requirements
must move together; the test suite enforces this.

## Pull requests

When a PR changes any file in a pack's manifest closure (`pack.toml`, query,
template, or WASM grammar), bump that pack's version in the same commit. New
packs begin at `0.1.0`. Changes to non-distributed notes do not need a bump.

CI runs:

```console
cargo run --locked --example check_pack_releases -- --base <base-commit>
```

It compares each changed closure with the base commit and rejects a version
that did not increase. It cannot decide whether a change is behaviorally
breaking; the author and reviewer must select the correct increment using the
policy above.

## Publishing

Pack tags are independent and immutable:

```text
pack/<manifest-name>/v<pack-version>
```

For example:

```console
git tag -a pack/rust-lexical/v0.2.0 -m "rust-lexical 0.2.0"
git push origin pack/rust-lexical/v0.2.0
```

The tag workflow validates that exactly one current manifest has that name,
that its version equals the tag, that the pack loads, and that the repository
passes tests and clippy. It then creates the corresponding GitHub release.
Never move or delete a published tag; an old tag is the rollback source.

When one change releases several packs, create one tag per manifest name at
the same commit. In particular, release native/WASM twins together when their
shared behavior changes.

## Curated catalog contract

A curated installer should index releases per pack rather than assigning
SemVer meaning to a repository-wide snapshot. Each catalog entry must carry
the exact `name`, `version`, immutable `ref` and resolved commit, directory,
`btt` requirement, and per-file SHA-256 digests. A catalog snapshot may have
an opaque sequence or date, but `packs-v1` must not be interpreted as the
major version of every pack in it.

An updater should remain within the project's selected pack major by default.
Crossing a major (or a pre-1.0 minor) must require an explicit opt-in and show
the migration notes. Exact resolution belongs in a committed `btt.lock`; the
human configuration can eventually carry the permitted version range.

## Adopting packs in projects

Until lock/sync support lands, reproducible projects should vendor external
packs under `.btt/packs/<name>/`, commit those files, and pin the `btt` binary
version in CI. The project-local directory already has highest resolution
priority. User-global packs are convenient for evaluation, but should not be
the source used by reproducible CI.

Builtin packs are part of the `btt` binary, so pinning the binary pins them.
To adopt a breaking pack release, update it on a branch, run `btt check`,
review every new finding and scaffold change, perform the documented
migration, and commit the updated pack and pin together.
