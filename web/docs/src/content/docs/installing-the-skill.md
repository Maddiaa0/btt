---
title: Installing the skill + usage
description: Give Claude Code, Codex, or another coding agent the project's tree-first test rules.
---

The btt skill is a short set of project instructions. It tells a coding agent to update the `.tree` file before changing tests, use btt to scaffold new test structure, and run `btt check` when it is done.

## Claude Code

Run this at the project root:

```sh
btt init --skill
```

It creates:

```text
your-project/
├── btt.toml
└── .claude/
    └── skills/
        └── btt/
            └── SKILL.md
```

Existing config and skill files are left alone. Commit the skill so it is available to other contributors and future sessions.

## Codex and other agents

The `--skill` flag currently writes a Claude skill. Codex reads repository instructions from `AGENTS.md`. Other agents have similar project files.

Copy the **Branch tree testing (btt)** section from [`SNIPPET.md`](https://github.com/Maddiaa0/btt/blob/main/SNIPPET.md) into the right file for your agent:

| Agent | Put the instructions here |
| --- | --- |
| Claude Code | `.claude/skills/btt/SKILL.md` |
| Codex | `AGENTS.md` |
| Other agents | Their repository instruction file |

[`AGENT-SETUP.md`](https://github.com/Maddiaa0/btt/blob/main/AGENT-SETUP.md) is different. It is a one-off prompt you paste into a chat when you want an agent to install btt and set up a project. Do not add that whole prompt to `AGENTS.md` or `CLAUDE.md`.

## Ask for the tree first

A useful request names the change and asks to see the cases before implementation:

```text
Add expiration handling to the token cache. Update the .tree file first and
show me the new cases. Once we agree on the tree, scaffold and implement the
tests, then run btt check and the normal test suite.
```

For a small, obvious change, the agent can do the whole sequence in one go:

```text
update .tree → scaffold or edit tests → implement → btt check → project tests
```

For a larger change, stop after the tree update and review it. Changing five lines in a tree is easier than unwinding an implementation built around the wrong cases.

## What btt checks, and what you check

btt can confirm that the tests promised by the tree exist in the right place. It cannot judge the test bodies.

When reviewing the result, check:

- Are success and failure cases both present?
- Is important setup shown as a `given …` branch?
- Does each `it …` line name one result?
- Does each test actually arrange the stated condition and assert that result?
- Did the agent change the plan for a good reason, or only to make the check pass?

:::caution
Do not delete a valid line from the tree just because its test is missing. Write the test, or agree that the expected behavior has changed.
:::

## Existing test suites

Migrate one file at a time:

1. Ask the agent to read the tests and list what they currently cover.
2. Write a `.tree` beside the test file.
3. Group flat, hard-to-read test names into useful `given …` and `when …` branches.
4. Run `btt check` and fix the differences.
5. Review the final tree before moving on.

btt does not generate trees backwards from test code. Writing the tree is where you notice duplicated, missing, and badly named cases.

Keep `uncovered = "warn"` during the move. Change it to `"error"` after every test file has a tree.

## Finish with both checks

An agent that changes tests should run btt and the test runner:

```sh
btt check
cargo test        # replace with the project's test command
```

`btt check` exits with code `1` when the tree and tests differ, and code `2` when a file cannot be checked. Both should fail CI.
