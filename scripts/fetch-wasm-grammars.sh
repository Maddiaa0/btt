#!/usr/bin/env bash
# Fetch prebuilt tree-sitter WASM grammars for the packs-wasm/ packs.
#
# Grammars are pinned by release tag AND sha256 — the same supply-chain
# posture as Cargo.lock. The .wasm files are the exact artifacts published
# by the tree-sitter org, at the same grammar versions btt compiles in
# natively (so wasm and native extraction can be compared 1:1).
set -euo pipefail
cd "$(dirname "$0")/.."

fetch() {
  local repo="$1" tag="$2" asset="$3" dest="$4" sha="$5"
  if [[ -f "$dest" ]] && echo "$sha  $dest" | sha256sum -c --quiet 2>/dev/null; then
    echo "ok (cached)  $dest"
    return
  fi
  echo "fetching     $repo@$tag/$asset"
  curl -sfL "https://github.com/${repo}/releases/download/${tag}/${asset}" -o "$dest"
  echo "$sha  $dest" | sha256sum -c --quiet
  echo "ok (fetched) $dest"
}

fetch tree-sitter/tree-sitter-rust v0.24.2 tree-sitter-rust.wasm \
  packs-wasm/rust/grammar.wasm \
  "24c89bd9252255e4aebbcbd7d2d308bd92c86dd95a130fdc80efa49577b8d738"

fetch tree-sitter/tree-sitter-typescript v0.23.2 tree-sitter-typescript.wasm \
  packs-wasm/typescript/grammar.wasm \
  "778025db5a8be0e70f8ccc3671e486dfeddd048c25d9e8a70c26de2e1bf6f97d"
