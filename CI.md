# Continuous integration

[CI](.github/workflows/ci.yml) runs on every push to `main`, when started by
hand, and for pull requests through [PR CI](.github/workflows/pr-ci.yml), without
path filters. Its five jobs check Python, JavaScript/TypeScript and Rust
independently and run the native and browser suites.

## Pull requests

PR CI tests a pull request into `main` on request:

| Event | What PR CI does |
| --- | --- |
| A pull request opens ready, or a draft is marked ready | Runs, for a collaborator's pull request |
| `ci:run` label | Runs on the latest commit, for anyone's pull request; the label is removed |
| A push to a ready pull request | Cancels the run in progress; the new commit waits for `ci:run` |

The result is the `CI required` commit status on the pull request's head:
pending while untested or running, then passed or failed, linked to the run.
There is no per-author run limit: runs use GitHub's hosted runners, which cost
nothing for a public repository.

GitHub runs `main`'s copy of PR CI, so a pull request cannot change when it is
tested or what status it gets. The checks run the pull request's code with a
read-only token. Because that code runs in `main`'s cache scope, the checks
restore caches but never save them, and an outside contributor's pull request
runs only after a maintainer reads it and adds `ci:run`.

Runs on `main` save the caches. GitHub shares the default branch's caches with
pull requests, so each push to `main` keeps them warm for every pull request.
Pull request runs keep coverage in the job summary and upload no artifacts.

## Planned structure

This is the agreed design for what comes next; each part replaces the
description above as it lands.

- **`main` is protected.** Merges require `CI required`, and rebase is the only
  merge method.
- **Build once, fan out.** One build job produces the heavy outputs, the
  WebAssembly emulator, the JavaScript bindings and the Python native extension,
  and the test jobs download them instead of compiling their own. Rust compiles
  go through a compiler cache, and each output restores only on the exact key of
  the inputs it reads.
- **`main` publishes its outputs.** Each passing `main` commit uploads those
  outputs as artifacts named by commit, for builds that pin a FujiMod commit to
  download instead of compiling; they build from source when no artifact exists.
  Uploads expire after a few days, a cleanup step after each `main` run deletes
  all but the newest five commits' outputs, and their total stays under a fixed
  size cap below the account's artifact storage.
- **The nightly web deploy ships the newest `main` commit that passed CI,**
  reusing its outputs, instead of running the checks itself. A broken check
  fails the pull request that broke it rather than blocking deploys.

## Language policy

- Python uses Astral's [ty](https://docs.astral.sh/ty/) and
  [Ruff](https://docs.astral.sh/ruff/linter/), installed through the root uv lock.
  Ruff requires annotations on every function's parameters and return type,
  including private functions and tests, and rejects explicit `Any` annotations.
  There is no allowance for a percentage of untyped functions. ty checks all
  first-party Python with warnings treated as errors and additional checks for
  missing generic arguments, unsound assignments/returns/yields and blanket or
  unused suppressions. This follows Astral's
  [stricter checking guidance](https://docs.astral.sh/ty/coming-from-mypy-or-pyright/#stricter-checking-with-ty).
  ty does not provide a numeric type-coverage report; these gates enforce complete
  function signatures and type correctness, rather than claiming every expression
  has a statically known type. Third-party native/Pyodide interfaces need accurate
  stubs or typed protocols; disabling import diagnostics would conceal gaps.
- Ruff also checks Pyflakes, pycodestyle's syntax/error groups, pyupgrade,
  bugbear, simplify, import sorting and stub hygiene. Ruff formats Python.
  The package's minimum is Python 3.14, including template-string support;
  repository tooling and native tests run on the Python 3.14 pinned in Mise.
- JavaScript/TypeScript uses [Vite+](https://viteplus.dev/guide/lint) with its
  Oxlint correctness rules, type-aware rules, full TypeScript checking and
  formatting. The web app also retains its existing Solid-specific rules.
  The root configuration covers bindings, scripts, simulation tools and USB
  sharing; generated Wasm, distributions and vendored dependencies are excluded.
- Rust uses rustfmt and the standard [Clippy](https://doc.rust-lang.org/clippy/)
  lint set across every workspace target and feature, with warnings denied.
  Pedantic/restriction lints are not enabled wholesale.
- `rust-hack-check` fails when the generated `fujimod-workspace-hack` crate no
  longer matches the dependency graph; `rust-hack` regenerates it.

No violation baselines, changed-lines-only filters or test-file typing exemptions
are used. `python-typecheck` builds the JavaScript binding and generates Python
protocols from its checked TypeScript declarations, then generates service value
declarations from Rust. Generated types stay in `target/python-types`. The
`typescript-api` dependency provides the TypeScript compiler API; TypeScript 7
checks maintained source. `typing_contract.py` checks quantity boundaries, enum
groups, immutable properties and asynchronous return types.

## Run locally

From the repository root:

```sh
mise install
uv sync --locked
pnpm install --frozen-lockfile
pnpm --filter fujimod run setup:tooling

mise run python-lint
mise run python-format-check
mise run python-typecheck
mise run rust-format-check
mise run catalog-format-check
mise run rust-check
pnpm --filter fujimod run build
mise run javascript-lint
mise run web-check
mise run javascript-format-check
```

The emulator crate builds and statically links its pinned Unicorn source.
The default Python emulator feature also needs libjpeg development headers.

`mise run python-test` builds the native binding, then runs pytest discovery
throughout the repository. This includes Python bindings, Wasm transport tests,
simulation asset preparation and CI tooling.
Install ffmpeg to exercise walkthrough media tests. Tests use scripted USB
transcripts and peers, not physical cameras.

`mise run javascript-node-test` runs the proxy/exporter, asset staging and Worker
tests, then rebuilds and tests both instrumented and production Wasm bindings.
It continues through independent suites after a failure and returns failure if
any suite failed. `mise run rust-wasm-test` additionally executes the Rust USB
core tests in the JavaScript runtime. The setup command installs the matching
`wasm-bindgen-test-runner` as well as the code generator.

The browser job installs Chromium and WebKit and runs the WebUSB browser
contracts, Pyodide contracts in Node and Chromium, web unit/component tests,
production build, prerender checks, developer-tools browser checks and Worker
build validation. Pyodide also needs its pure Python wheel:

```sh
node bindings/python/scripts/build-web-wheel.ts --out target/web-wheels
pnpm --filter fujimod-web exec -- playwright install --with-deps chromium webkit
mise run javascript-browser-test
pnpm --filter fujimod run test:pyodide:browser
mise run web-coverage
mise run web-build
node scripts/ci/package-tests.ts apps/web
```

The web integration runner executes every declared `test:*` package script
except interactive `*:watch` commands, so new one-shot suites join CI when added.

Browser scripts that take a firmware update file, a simulator bundle, a served
camera session or a connected camera run only when given those inputs; CI does
not run them.

## Coverage

Test coverage is reported, without a minimum percentage imposed. The 100%
requirement above concerns Python typing, not execution coverage.

- pytest-cov records Python line and branch coverage, XML, HTML and JUnit results.
  It lists unexecuted Pyodide adapters as uncovered; separate Pyodide contract
  runs are not included in that measurement.
- cargo-llvm-cov records the entire native Rust workspace. Python bindings are
  built and exercised with the same LLVM instrumentation, so their native
  calls contribute to the report. Documentation tests run separately. Stable
  Rust coverage does not include the separately executed Wasm tests.
- Vitest's V8 provider reports web line and branch coverage, including unimported
  source files, as text, LCOV, HTML and JSON summaries.
- Node's test runner emits LCOV and console reports for each standalone suite.
  These measure host JavaScript. Browser code, Wasm, dynamically imported data
  URLs and Cloudflare Worker isolates are not measured by host V8 coverage;
  an empty report is displayed as `0/0`, never as proof of 100% coverage.

Artifacts and job summaries are published even when tests fail. Each Actions run
offers `native-coverage` and `javascript-coverage` downloads; generated files stay
under ignored `target/coverage/`. To reproduce native coverage:

```sh
export CARGO_TARGET_DIR="$PWD/target/llvm-cov-target"
export CARGO_LLVM_COV_TARGET_DIR="$CARGO_TARGET_DIR"
mise run rust-coverage
source <(cargo llvm-cov show-env --sh)
mise run python-test
mkdir -p target/coverage/rust
cargo llvm-cov report --lcov --output-path target/coverage/rust/lcov.info
cargo llvm-cov report --html --output-dir target/coverage/rust
uv run --locked python scripts/ci/coverage_summary.py
```
