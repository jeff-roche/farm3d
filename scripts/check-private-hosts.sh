#!/usr/bin/env bash
# Fails if a tracked file, or anything staged for commit, names one of the
# repo owner's private hosts: real printer addresses, hostnames, serials.
#
# The denylist deliberately lives outside the repository, so this script
# never has to name what it guards:
#
#   FARM3D_PRIVATE_HOSTS   space- or comma-separated fixed strings
#   .private-hosts         one fixed string per line (gitignored; # comments)
#
# With neither set there is nothing to check, and it says so. Use RFC 5737
# addresses (192.0.2.x, 198.51.100.x, 203.0.113.x) in docs and examples.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

patterns=()
if [ -n "${FARM3D_PRIVATE_HOSTS:-}" ]; then
    read -r -a from_env <<<"${FARM3D_PRIVATE_HOSTS//,/ }"
    patterns+=("${from_env[@]}")
fi
if [ -f .private-hosts ]; then
    while IFS= read -r line; do
        line="${line%%#*}"
        line="$(echo "$line" | xargs)"
        [ -n "$line" ] && patterns+=("$line")
    done <.private-hosts
fi

if [ "${#patterns[@]}" -eq 0 ]; then
    echo "check-hosts: no denylist (set FARM3D_PRIVATE_HOSTS or create .private-hosts); nothing checked"
    exit 0
fi

found=0
for pattern in "${patterns[@]}"; do
    # Print only file:line, never the matching text.
    hits="$(git grep -n -I -F -e "$pattern" -- . ':!.private-hosts' | cut -d: -f1,2 || true)"
    if [ -n "$hits" ]; then
        echo "$hits" | sed 's/^/  tracked: /'
        found=1
    fi
    if git diff --cached -U0 | grep -q -F -e "$pattern"; then
        echo "  staged: a denylisted host is in the staged changes"
        found=1
    fi
done

if [ "$found" -ne 0 ]; then
    echo "check-hosts: FAIL: private host names found (locations above)" >&2
    exit 1
fi
echo "check-hosts: ok (${#patterns[@]} denylisted strings, none found)"
