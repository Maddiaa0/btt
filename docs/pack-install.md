# Spec: pack installation (`btt pack`)

Design for the pack install/list/show/rm surface, resolving the
installation-and-lifecycle portion of issue #10. Scope is one
implementable unit; deferred items are listed at the end.

## Principles

1. **Installing never executes pack code.** An install is: acquire files,
   validate them with the same loader `btt check` uses, copy an allowlist
   into place. No hooks, no subprocesses from pack content, no archive
   formats.
2. **The user reviews what they install.** Third-party installs show the
   actual pack content (lexical packs are one screen of text) and require
   explicit confirmation. Wasm blobs cannot be reviewed by eye, so they
   are surfaced as such — digest, size, provenance — with a warning.
3. **Official packs are frictionless because they are pre-verified, not
   because checks are skipped.** The curated index ships inside the btt
   binary with per-file sha256 digests; the fetch is by immutable git
   tag; every byte is verified against digests that were reviewed in this
   repo before release. Same trust root as the binary itself.
4. **Install does not activate.** Packs only run when `btt.toml` names
   them (`packs = [...]`). Install prints the activation hint and never
   edits `btt.toml`.

## CLI surface

```
btt pack list                 # installed + builtin packs (alias: btt packs)
btt pack show <name>          # manifest summary, files, digests, provenance
btt pack install [<name>]     # curated selector (official repo, pinned);
                              # a name skips the selector (CI-friendly)
btt pack install --git <url> [--ref <branch-or-tag>] [--dir <subdir>]
btt pack install --path <dir>
btt pack rm <name>
```

Shared install flags:

- `--project` — install into `<project>/.btt/packs/` (vendored) instead
  of the user-global `$XDG_CONFIG_HOME/btt/packs/` (default
  `~/.config/btt/packs/`). The legacy `~/.btt/packs/` is read-only
  compat; the installer never writes there.
- `--force` — overwrite an already-installed pack of the same name.
- `--yes` — skip the interactive confirmation (for scripts/CI). Without
  it, a non-interactive stdin + a non-curated source fails closed.

`btt packs` remains as an alias for `btt pack list`.

## Install pipeline (all sources)

Every source funnels through the same stages. Only stage 1 differs.

### 1. Acquire (into a read-only temp checkout)

- **Curated (default):** `git clone --depth 1 --branch <pinned-tag>` of
  the official repo into a temp dir, using the tag recorded in the
  embedded index (see below). Requires system `git`, same as `--git`.
- **`--git`:** `git clone --depth 1` (plus `--branch <ref>` when given)
  into a temp dir. The checked-out commit hash is captured for the
  receipt. Note: `git clone` runs no repo-provided hooks.
- **`--path`:** the directory is used in place, never modified.

### 2. Discover

Scan the checkout for `pack.toml` files (bounded depth, e.g. 3). One
found pack installs directly; multiple pack dirs present a numbered
selection (plain stdin prompt — no TUI dependency). The curated flow
skips scanning: the embedded index names each pack's directory.

### 3. Stage — allowlist copy

Parse `pack.toml` (strict, `deny_unknown_fields`, existing code) and copy
**only** the manifest closure into a staging dir under the destination
packs root (`<packs-root>/.staging/<name>.<random>` — same filesystem so
the final rename is atomic):

- `pack.toml`
- `extract.query`
- `scaffold.template`
- the `wasm:` grammar file, if any

Nothing else is copied — no stray files ride along. Per-file rules:

- Refuse symlinks and non-regular files (`symlink_metadata`, open with
  `O_NOFOLLOW` semantics: read via the resolved-and-checked path).
- Per-file size cap: 8 MiB (wasm grammars run ~1–2 MiB). Text files
  (manifest, query, template): 256 KiB. Caps are constants with tests.
- Compute sha256 of each file as it is copied (for the receipt, and for
  curated verification).

### 4. Validate the staged copy

Run `pack::load_dir()` on the staging dir — the real loader: strict
manifest, `confine` on every path field, `resolve_inside` symlink
confinement, target-pattern reversibility. Anything the loader rejects
aborts the install with the loader's own error. There is no separate
"shape checker" to drift out of sync.

For curated installs additionally verify each staged file's sha256
against the embedded index; any mismatch aborts with a supply-chain
error (this should never happen and is worded accordingly).

### 5. Identity and collision checks

- Final directory name is `manifest.pack.name` — not the source folder
  name (the resolver keys on directory name; a mismatch would install a
  pack that validates but never loads).
- The name must be a single `Component::Normal` (same check as
  `pack::load`); otherwise abort.
- Name collides with an installed pack in the target root → error unless
  `--force`.
- Name collides with a builtin (`rust`, `typescript`) → the install
  proceeds (shadowing is legitimate and opt-in at activation), but a
  prominent warning always prints, even under `--yes`.

### 6. Review gate

Printed for every install, before the confirmation prompt:

- Manifest summary: name, version, description, grammar kind
  (`lexical` / `wasm` / `builtin:` reference).
- File table: path, size, sha256 (short).
- Provenance: source kind + URL + resolved commit/tag.

Then, by source:

- **Curated:** confirmation of the selection only ("install <name>?
  [Y/n]") — content is pre-verified against the embedded digests.
- **`--git` / `--path`:** the text files (manifest, query, template) are
  printed in full — a lexical pack's entire definition fits on one
  screen — followed by "install? [y/N]" (default no). A wasm grammar is
  never printed; instead: `contains a binary grammar blob (<size>,
  sha256 <digest>) — btt cannot review this for you; install only from
  sources you trust`.

### 7. Receipt

`receipt.toml` written into the staged pack dir before the rename:

```toml
[install]
source = "curated" | "git" | "path"
url = "https://github.com/Maddiaa0/btt"   # git/curated
ref = "v0.3.0"                             # as requested, if any
commit = "<resolved hash>"                 # git/curated
installed-by = "btt <version>"
date = "<RFC 3339>"

[files]
"pack.toml" = "sha256:..."
"queries/tests.scm" = "sha256:..."
"templates/test.jinja" = "sha256:..."
```

The loader ignores it (loaders read only manifest-referenced files).
`pack show` and `pack list` read it; it is what makes a future
`pack update`, audit, and CI lockfile checks possible.

### 8. Commit — atomic rename

- No existing target: `rename(staging, <packs-root>/<name>)`.
- With `--force`: `rename(existing, .trash/<name>.<random>)`, then
  `rename(staging, target)`, then delete trash. If the second rename
  fails, the trash rename is reverted — the old pack is never lost to a
  half-completed swap.
- Any abort deletes the staging dir; `.staging/` leftovers from crashed
  runs are swept opportunistically on the next install.

On success print the activation hint:
`installed <name> <version> to <path>` +
`activate it by adding "<name>" to packs = [...] in btt.toml`.

## The curated index

Embedded in the binary at build time (`include_str!` of
`packs-index.toml` at the repo root), containing for each offered pack:

```toml
tag = "packs-v3"          # immutable release tag the files live at

[[pack]]
name = "solidity"
kind = "lexical"
description = "Foundry/Hardhat test conventions"
dir = "packs-lexical/solidity"
files = { "pack.toml" = "sha256:...", "queries/tests.scm" = "sha256:..." }
```

- `btt pack install` with no flags lists this index (name, kind badge,
  description) and installs the selection via the pipeline above.
- Regenerated by `scripts/gen-packs-index.sh` (a thin wrapper around
  `cargo run --example gen_packs_index`) when packs change; a freshness
  test in `tests/install.rs` fails CI when the committed index does not
  byte-match a regeneration, so a stale index cannot ship.
- Only packs whose **entire closure is tracked by git** are included —
  the generator skips the rest loudly. The wasm twins in `packs-wasm/`
  reference grammar blobs that live in release assets, not git, so the
  index starts empty; it lights up when the lexical packs (PR #12) land.
- Tradeoff accepted: new curated packs require a btt release. `--git`
  against the repo covers the gap in the meantime.

Builtins (`rust`, `typescript`) do not appear in the index — they are
already in the binary; `pack list` shows them as `builtin`.

## `pack list` / `pack show`

`list`: every visible pack — builtin, user-global (XDG + legacy),
project-local — with name, version, kind, origin, and short digest of
`pack.toml`; when run inside a project, an `active` marker for packs
named in `btt.toml` and a `shadows builtin` / `shadows user` marker when
resolution order hides another pack of the same name. This extends the
existing `cmd_packs`.

`show <name>`: resolves like `pack::load` (same order), prints the
review-gate view (manifest summary, file table with digests, receipt
provenance if present) without installing anything.

## `pack rm <name>`

- Name must be a single normal path component (reuse the `load` check).
- Builtin name with no installed pack of that name → error: compiled
  into the binary, cannot be removed.
- Targets the user-global XDG root by default; `--project` targets
  `<project>/.btt/packs`. The legacy `~/.btt/packs` is only touched when
  the pack exists **only** there, and rm says so.
- The path must be a real directory (not a symlink) physically inside
  the chosen packs root; otherwise refuse.
- Prints what will be deleted (path + pack version from its manifest if
  parseable) and confirms (`--yes` skips); then `remove_dir_all`.
- If the same name is also visible in another root afterward, print
  which pack now wins resolution.

## Security invariants (each becomes a test)

1. Install executes no pack-provided code and parses no archive formats.
2. Only the manifest closure is copied; a junk/extra file in the source
   never reaches the packs root.
3. A symlink anywhere in the source aborts the install (never followed,
   never copied).
4. Oversized files abort (per-file caps for text and wasm).
5. Validation runs on the staged copy, after copying — a source mutated
   mid-install cannot bypass it (no TOCTOU).
6. The installed directory name always equals `manifest.pack.name`, and
   that name is a single normal path component.
7. Curated installs verify per-file sha256 against digests embedded in
   the binary; mismatch aborts.
8. `--git` resolves to a commit hash recorded in the receipt.
9. Non-curated installs require interactive confirmation or `--yes`;
   non-interactive without `--yes` fails closed.
10. Install never edits `btt.toml`; an installed-but-unnamed pack never
    executes (existing activation invariant, retested end-to-end).
11. `rm` deletes only a non-symlink directory inside a packs root, never
    resolves a name containing path separators, never removes builtins.
12. Interrupted installs leave the packs root either unchanged or fully
    updated (staging + rename; `--force` swap is revert-safe).

## Test plan (tree-first, per CLAUDE.md)

All installer logic lives in a library module (`src/install.rs` +
`src/install.tree` for its unit tests), so it tests without a terminal.
`tests/install.rs` + `tests/install.tree` cover the invariant list
end-to-end against fixture directories: allowlist staging, symlink and
size-cap refusal, staged-copy validation, receipts, collision and
`--force` swap, drop cleanup, curated digest verification, git
acquisition via a local `file://` remote, all rm guards, and the
index-freshness check.

## New dependencies

- `sha2` (digests) — added via `cargo +nightly add` under the 14-day
  min-publish-age policy; lockfile audit clean.
- No HTTP client, no TUI, no archive crates. Acquisition uses system
  `git` (documented requirement; already required by `--git` semantics).
- Timestamps for receipts use `std::time::SystemTime` formatted
  manually or a small pre-existing dep only if already in-tree — no new
  date crate.

## Deferred (recorded, not built now)

- `pack update` (receipts make it possible), yanked/revocation story,
  registry beyond the official repo, CI lockfile enforcement mode,
  subprocess isolation for hostile wasm, auto-activation prompts.
