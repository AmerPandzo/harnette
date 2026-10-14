#!/usr/bin/env bash
# Creates a small throwaway repo with task docs ready for each manual check in
# .ai-tasks/phase-1-test-steps.md. Usage: scripts/playground.sh [dir]   (default /tmp/harnette-playground)
set -euo pipefail
dir="${1:-/tmp/harnette-playground}"
if [ -e "$dir" ]; then
  echo "$dir already exists; remove it first (rm -rf \"$dir\") or pass another path." >&2
  exit 1
fi
mkdir -p "$dir/src" "$dir/.ai-tasks"
cd "$dir"
git init -q -b main
printf '.ai-tasks/\n' >> .git/info/exclude
commit() { git -c user.name="Harnette Playground" -c user.email=playground@example.com commit -qm "$1"; }

cat > package.json <<'EOF'
{ "name": "harnette-playground", "type": "module", "scripts": { "test": "node --test" } }
EOF
cat > src/math.js <<'EOF'
// Returns the absolute difference between a and b.
export function distance(a, b) {
  return Math.abs(a - b);
}
EOF
cat > src/math.test.js <<'EOF'
import { test } from "node:test";
import assert from "node:assert/strict";
import { distance } from "./math.js";

test("distance", () => {
  assert.equal(distance(2, 5), 3);
});
EOF
git add -A && commit "Initial playground"

cat > src/double.js <<'EOF'
export function double(n) {
  return n * 2;
}
EOF
cat > src/double.test.js <<'EOF'
import { test } from "node:test";
import assert from "node:assert/strict";
import { double } from "./double.js";

test("double", () => {
  assert.equal(double(4), 8);
});
EOF
git add -A && commit "PLAY-4: double helper (written by hand)"

task() { # slug status title problem fix
  cat > ".ai-tasks/$1.md" <<EOF
# $1 — $3

**Status:** $2

## Problem

$4

## Analysis

\`src/\` has no such helper yet.

## Conclusion

Add it with tests.

## Proposed fix

$5

## Acceptance criteria

- [ ] The helper behaves as described in Problem, including the edge cases named there
- [ ] \`npm test\` passes

## Test plan

\`npm test\`.

## Out of scope / constraints

Do not change \`src/math.js\`.

## Open questions

None.

## Implementation notes

## Review log
EOF
}
task PLAY-1 "Ready for implementation" 'add `clamp(n, lo, hi)`' \
  'We need `clamp(n, lo, hi)`: returns n limited to the range [lo, hi]. Throws `RangeError` when lo > hi.' \
  '1. Create `src/clamp.js` exporting `clamp`.
2. Create `src/clamp.test.js` covering below, inside, above and lo > hi.'
task PLAY-3 "Ready for implementation" 'add `average(list)`' \
  'We need `average(list)`: the arithmetic mean of an array of numbers. An empty array throws `RangeError`.' \
  '1. Create `src/average.js` exporting `average`.
2. Create `src/average.test.js` covering a normal list, a single item and the empty array.'
task PLAY-4 "In review" 'add `double(n)`' \
  'We need `double(n)` returning n * 2. Already implemented by hand and committed.' \
  '1. `src/double.js` exporting `double`.
2. `src/double.test.js`.'
task PLAY-5 "Ready for implementation" 'add `sign(n)`' \
  'We need `sign(n)` returning -1, 0 or 1.' \
  '1. Create `src/sign.js` exporting `sign`.
2. Create `src/sign.test.js` covering -5, 0 and 7.'

cat > .ai-tasks/PLAY-2.md <<'EOF'
# PLAY-2 — add `percent(part, whole)`

**Status:** Analysis

## Problem

We need `percent(part, whole)` returning part as a percentage of whole.

## Analysis

`src/` has no such helper yet. Rounding and division by zero are unspecified.

## Conclusion

Needs two decisions before implementation.

## Proposed fix

1. Create `src/percent.js` exporting `percent`.
2. Create `src/percent.test.js`.

## Acceptance criteria

- [ ] `percent(1, 4)` returns 25
- [ ] `npm test` passes

## Test plan

`npm test`.

## Out of scope / constraints

Do not change `src/math.js`.

## Open questions

1. How many decimals should the result be rounded to?
2. What should `percent(1, 0)` do: throw, or return `Infinity`?

## Implementation notes

## Review log
EOF
echo "Playground ready: $dir"
echo "Tasks: PLAY-1 (Ready), PLAY-2 (Analysis), PLAY-3 (Ready), PLAY-4 (In review, committed by hand), PLAY-5 (Ready)"
