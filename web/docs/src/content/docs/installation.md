---
title: Installation
description: Install btt with Cargo and set up a project.
---

btt is currently installed from GitHub with Cargo.

## Requirements

You need Rust 1.85 or newer, Cargo, and Git.

```sh
rustc --version
cargo --version
git --version
```

If Rust is missing, install it with [rustup](https://rustup.rs/).

## Install btt

```sh
cargo install --git https://github.com/Maddiaa0/btt
```

Cargo normally puts the binary in `~/.cargo/bin`. Check that your shell can find it:

```sh
btt --version
btt --help
```

If it cannot, add Cargo's binary directory to `PATH` and open a new shell.

## Install from a checkout

Use a local checkout if you are working on btt itself or want to inspect the exact code before installing it:

```sh
git clone https://github.com/Maddiaa0/btt.git
cd btt
cargo install --path .
```

Run the last command again after pulling a newer revision.

## WASM grammar support

The normal build includes Rust and TypeScript parsers and supports lexical packs. You only need the `wasm` feature for a pack that ships a tree-sitter WASM grammar:

```sh
cargo install --git https://github.com/Maddiaa0/btt --features wasm
```

WASM grammars cannot use WASI to access the filesystem or network. They are still executable parser input, so only install grammars you trust and pin the version and digest.

## Set up a project

Run `init` at the project root:

```sh
cd your-project
btt init
```

This writes `btt.toml` with the Rust pack selected. It leaves an existing config alone.

To also add the Claude Code skill:

```sh
btt init --skill
```

See [Installing the skill + usage](/installing-the-skill/) for other agents.

## Update or remove

Install the current Git revision over the existing binary:

```sh
cargo install --git https://github.com/Maddiaa0/btt --force
```

Remove btt:

```sh
cargo uninstall btt
```

Uninstalling the binary does not remove `btt.toml`, `.tree` files, or project skills.
