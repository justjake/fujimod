# Working in FujiMod

Use TypeScript for all maintained browser, worker, Node, build and test source
(`.ts`, `.mts`, `.cts`, `.tsx`). Do not add JavaScript source files or executable
inline HTML scripts. Generate browser/wasm-bindgen JavaScript only in ignored
build directories. Keep types in the implementation rather than handwritten
`.d.ts` facades for untyped code.

Every TypeScript configuration must explicitly disable `allowJs`, including
inherited effective settings. Never pass `--allowJs` to tooling. Check source,
scripts and tests without `@ts-nocheck`, `@ts-ignore`, blanket `any` or weakened
compiler checks. Run `pnpm run check:typescript-policy` and the affected type
checks and tests before committing. The policy check runs in CI.

Checked-in generated files carry `.generated` before their extension
(`control-ids.generated.json`). Never edit them by hand: change their inputs or
generator and regenerate. Their checks fail when the checked-in copy differs
from a fresh generation. Never mix generated and hand-authored content in one
file: a generator writes whole files, and hand-authored facts live in separate
files that generators only read.

Never put binary data larger than 128 bytes in JSON, whether as base64, hex or
number arrays. Write it to a separate binary file and reference that file by path from the JSON.

## Checkouts and commits

When FujiMod is checked out inside another repository, follow that
repository's commit workflow. Otherwise work on a topic branch and open a pull
request against `main`. In a checkout several agents share, commit with
`mise run commit -- -m "Describe the change" PATHS...` (`scripts/git/commit.py`)
rather than staging by hand.

GitHub Actions workflows run only on `workflow_dispatch`, started by a person.
Do not add `push`, `pull_request`, `schedule` or other automatic triggers.
