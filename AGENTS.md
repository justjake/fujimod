# Working in FujiMod

Use TypeScript for all maintained browser, worker, Node, build and test source
(`.ts`, `.mts`, `.cts`, `.tsx`). Do not add JavaScript source files or executable
inline HTML scripts. Generate browser/wasm-bindgen JavaScript only in ignored
build directories. Keep types in the implementation rather than handwritten
`.d.ts` facades for untyped code.

Every TypeScript configuration must explicitly disable `allowJs`, including
inherited effective settings. Never pass `--allowJs` to tooling. Check source,
scripts and tests without `@ts-nocheck`, `@ts-ignore`, blanket `any` or weakened
compiler checks. Run `npm run check:typescript-policy` and the affected type
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

Work, test and commit in the existing checkout on `main`. Do not create Git
worktrees, clones or branches; work left in them does not return to `main`, and
each copy duplicates large build output. Other contributors' edits in the same
checkout are not a reason to isolate: report tests they break instead.

The owner may approve one specific worktree in the conversation. Only then
create it on its own branch, with `FUJIMOD_WORKTREE_APPROVED="owner approved:
purpose"` set for `git worktree add -b worktree/PURPOSE PATH origin/main`;
without it the `post-checkout` hook removes the new worktree. An approval covers
one worktree and its branch and does not carry over to other agents or
conversations. Approved creations are logged in `.git/fujimod-worktrees.log`.
The hooks are installed with `mise run hooks-install`. Inside the worktree,
commit to its branch with plain `git add` and `git commit` at each milestone.
Land it with `mise run land` (`scripts/git/land.py`) from the worktree: it
rebases the branch onto `origin/main`, pushes `main` as a fast-forward and syncs
the main checkout. Once landed, remove the worktree and delete its branch.

Commit with `mise run commit -- -m "Describe the change" path/one path/two`
(`scripts/git/commit.py`). It catches up with `origin/main` (merging upstream
changes into local edits, `scripts/git/sync.py`), commits only the named paths
from a private index, pushes, and resets the shared index to the new `HEAD`. Only
the catch-up writes files, and only origin's changes. For a file that also holds someone else's edits,
pass a diff of only your hunks with `--patch FILE`. The shared index only
mirrors `HEAD`: do not stage, `git commit`, `merge`, `rebase`, `stash` or
`pull` in a shared checkout.

GitHub Actions workflows run only on `workflow_dispatch`, started by a person.
Do not add `push`, `pull_request`, `schedule` or other automatic triggers.
