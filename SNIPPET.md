<!-- Paste the block below into CLAUDE.md / AGENTS.md. These are permanent
     behavioral instructions for agents, not a setup script — for one-time
     installation, paste AGENT-SETUP.md into a chat with your agent instead.
     Claude Code users don't need this block: `btt init --skill` installs a
     richer skill at .claude/skills/btt/SKILL.md. -->


## Branch tree testing (btt)

Test suites in this project are specified as `.tree` files (given/when/it
branch trees) and kept in sync with the `btt` CLI. When writing, modifying,
or reviewing tests:

- **Spec first.** Add or update the `.tree` file before touching test code —
  new behavior is a new `when …`/`given …` branch with `it …` leaves. Never
  add a test the tree doesn't describe.
- **Scaffold, don't hand-write skeletons.** `btt scaffold path/to/name.tree`
  generates the test file with correctly named blocks and empty test bodies
  (`--stdout` to preview, `--force` to overwrite). Don't rename or
  restructure the generated blocks and tests — the names encode the tree.
- **Verify before finishing.** Run `btt check` before ending any task that
  touched tests; it must exit clean. While iterating, scope it to what you
  touched — it accepts `.tree` files and directories:
  `btt check src/map.tree src/util/`. Fix mismatches by updating the tree (if
  behavior legitimately changed) or the tests — never by deleting spec lines
  just to silence the check.
