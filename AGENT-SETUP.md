# Agent-assisted setup

Copy the block below and paste it into your coding agent (Claude Code, Codex,
etc.) from a checkout of this repo. It will install `btt`, initialize the
project you want tree-checked, and wire the tree-first workflow into your
agent's instructions.

> Note: this is a **one-time setup prompt** to paste into a chat — don't put
> it in `CLAUDE.md`/`AGENTS.md`. The instructions that belong in those files
> (telling agents to work tree-first) live in [SNIPPET.md](SNIPPET.md) and in
> the Claude skill that `btt init --skill` installs; steps 2–3 below set them
> up for you.

```text
Set up btt (branch tree testing — test suites specified as .tree files that
test code is checked against) from this repo:

1. Install the binary using the current platform's command from the README.
   If Rust is already available, `cargo install btt-cli --locked` is also
   supported. Verify the installed command with `btt --help`.

2. Initialize the project I want checked (ask me which repo if it isn't
   obvious): run `btt init --skill` at its root. This writes btt.toml and a
   Claude skill at .claude/skills/btt/SKILL.md. Then run `btt packs` and set
   [project].packs in btt.toml to the languages the project actually uses
   (e.g. ["rust"] or ["typescript"]).

3. If I use agents that don't read Claude skills (Codex, etc.): append the
   "Branch tree testing (btt)" block from SNIPPET.md in this repo to that
   project's AGENTS.md. Skip this for Claude Code — the skill from step 2
   already covers it. Don't duplicate the block if it's already there.

4. Show me it works: pick one small, well-understood test file in the
   project and write a .tree spec for it (same stem, next to the tests —
   see the installed SKILL.md for the full worker/reviewer/manager protocols).
   Explain that trees cover per-module unit tests, integration surfaces
   (e.g. tests/api.tree <-> tests/api.rs), and must-never-break properties in
   invariants.tree mapped to integration/property tests. If a guarantee matters
   enough to state in a PR description, it belongs as a leaf. Then run
   `btt check` and walk me through the output. Fix or explain its findings.

5. Suggest where to wire `btt check` into CI or a pre-commit hook (it exits
   non-zero on errors), but don't add it without asking me.

Report what you changed at each step.
```
