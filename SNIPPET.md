<!-- Paste the block below into CLAUDE.md / AGENTS.md. These are permanent
     behavioral instructions for agents, not a setup script — for one-time
     installation, paste AGENT-SETUP.md into a chat with your agent instead.
     Claude Code users don't need this block: `btt init --skill` installs a
     richer skill at .claude/skills/btt/SKILL.md. -->


## Branch tree testing (btt)

Test suites in this project are specified as `.tree` files (given/when/it
branch trees) and kept in sync with the `btt` CLI. The tree is shared
language between worker, reviewer, and manager.

- **Cover every tier.** Keep per-module unit trees; add one tree per
  integration surface (`tests/api.tree` ↔ `tests/api.rs`) for cross-module
  behavior; keep must-never-break properties in `invariants.tree`, mapped to
  integration/property tests. If a guarantee matters enough to state in a PR
  description, it belongs as a leaf.
- **Worker: spec first.** Add or update the `.tree` before touching test code —
  new behavior is a new `when …`/`given …` branch with `it …` leaves. Never
  add a test the tree doesn't describe or delete spec lines to go green.
  Expand parameterized cases into named leaves for behavioral distinctions;
  collapse pure data-point repetition into one representative leaf.
- **Scaffold, don't hand-write skeletons.** `btt scaffold path/to/name.tree`
  generates the test file with correctly named blocks and empty test bodies
  (`--stdout` to preview, `--force` to overwrite). Don't rename or
  restructure the generated blocks and tests — the names encode the tree.
- **Verify before finishing.** Run `btt check` before ending any task that
  touched tests; it must exit clean. While iterating, scope it to what you
  touched — it accepts `.tree` files and directories:
  `btt check src/map.tree src/util/`. Fix mismatches by updating the tree (if
  behavior legitimately changed) or the tests.
- **Reviewer.** Read `.tree` files first as the behavior inventory. Name a
  missing behavior as a `MISSING BRANCH` in tree syntax, and confirm the
  tree-diff matches the change's claimed scope.
- **Fix rounds and management.** Turn each accepted finding into named tree
  branches before code; report completion as the tree-diff. Workers quote and
  managers reproduce `N trees, 0 uncovered, 0 errors`. A green check scopes
  the reading; it never replaces review of the tree-diff, leaf assertions,
  load-bearing code, and a behavioral smoke test.
