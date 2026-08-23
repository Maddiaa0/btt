#!/usr/bin/env bash
# Regenerate packs-index.toml (the curated index embedded in the binary).
# Usage: scripts/gen-packs-index.sh [tag]
# The freshness test in tests/install.rs fails CI if this is stale.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo run --quiet --example gen_packs_index -- "$@"
