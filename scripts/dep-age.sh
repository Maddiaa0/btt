#!/usr/bin/env bash
# Audit Cargo.lock against crates.io publish dates: fail if any resolved
# crates.io dependency version was published less than MIN_AGE_DAYS ago.
#
# Usage: scripts/dep-age.sh [path/to/Cargo.lock]
#   MIN_AGE_DAYS=14   quarantine window (default 14)
set -euo pipefail

MIN_AGE_DAYS="${MIN_AGE_DAYS:-14}"
LOCK="${1:-Cargo.lock}"
UA="btt-dep-age-check (github.com/Maddiaa0/btt)"

if [[ ! -f "$LOCK" ]]; then
  echo "error: $LOCK not found" >&2
  exit 2
fi

now=$(date +%s)
violations=0
checked=0

# Cargo.lock package blocks list name, then version, then source; only
# blocks with a crates.io source are registry deps (path/workspace deps
# have no source line).
while read -r name ver; do
  # `|| true` keeps one rate-limited request from aborting the whole audit.
  created=$({ curl -sf --retry 3 --retry-delay 2 -H "User-Agent: $UA" \
    "https://crates.io/api/v1/crates/${name}/${ver}" | jq -r '.version.created_at'; } 2>/dev/null || true)
  if [[ -z "$created" || "$created" == "null" ]]; then
    echo "warn: could not fetch publish date for ${name}@${ver}" >&2
    continue
  fi
  created_epoch=$(date -d "$created" +%s)
  age_days=$(( (now - created_epoch) / 86400 ))
  checked=$((checked + 1))
  if (( age_days < MIN_AGE_DAYS )); then
    violations=$((violations + 1))
    echo "TOO FRESH  ${name}@${ver}  published ${age_days}d ago (< ${MIN_AGE_DAYS}d)"
  fi
  sleep 0.3 # stay well under crates.io rate limits
done < <(awk '
  /^name = /    { name = $3 }
  /^version = / { ver = $3 }
  /^source = .*crates.io/ {
    gsub(/"/, "", name); gsub(/"/, "", ver); print name, ver
  }
' "$LOCK")

echo "checked ${checked} crates.io packages, ${violations} younger than ${MIN_AGE_DAYS} days"
if (( violations > 0 )); then
  echo "fix: pin an older version with \`cargo update -p <crate> --precise <version>\`" >&2
  exit 1
fi
