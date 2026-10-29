#!/usr/bin/env bash
# The one check an agent runs before calling work done. It builds everything,
# runs the whole test suite and the no-host check, and refuses a change that
# only reformats a file. It exits non-zero on the first failure.
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1

fail() {
    echo "CHECK FAILED: $1" >&2
    exit 1
}

echo "== build"
cargo build --workspace 2>&1 | tail -3
[ "${PIPESTATUS[0]}" -eq 0 ] || fail "cargo build"

echo "== tests (the whole workspace)"
mkdir -p target/tmp
out=$(TMPDIR="$PWD/target/tmp" cargo test --workspace 2>&1)
status=$?
echo "$out" | grep -E '^test result|FAILED|panicked' | grep -v '0 passed; 0 failed' || true
[ "$status" -eq 0 ] || fail "cargo test (run it to see the failures)"
passed=$(echo "$out" | awk '/^test result/ {n += $4} END {print n}')
echo "$passed tests passed"

echo "== no-host check"
target/debug/sketch-client >/dev/null 2>&1 || fail "target/debug/sketch-client"

echo "== reformatted files"
# A changed Rust file that equals the last commit's version run through
# rustfmt was reformatted, not edited. Each is formatted inside a copy of its
# folder, so the modules it declares are found.
edition=$(grep -m1 -E '^edition' Cargo.toml | cut -d'"' -f2)
scratch=$(mktemp -d -p target/tmp)
noise=""
for file in $(git diff --name-only HEAD -- '*.rs'); do
    [ -f "$file" ] || continue
    git cat-file -e "HEAD:$file" 2>/dev/null || continue
    rm -rf "${scratch:?}"/*
    git archive HEAD "$(dirname "$file")" | tar -x -C "$scratch"
    rustfmt --edition "${edition:-2024}" "$scratch/$file" >/dev/null 2>&1 || continue
    if cmp -s "$scratch/$file" "$file"; then
        noise="$noise $file"
    fi
done
rm -rf "$scratch"
[ -z "$noise" ] || fail "reformatted only, restore with git checkout --$noise"

echo "== changed since the last commit"
git status --short
echo "All checks passed."
