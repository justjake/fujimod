# Continuous integration

[CI](.github/workflows/ci.yml) runs on every push and pull request, without path
filters. Its five jobs check Python, JavaScript/TypeScript and Rust independently
and run the native and browser suites. `CI required` fails if any job fails,
is canceled or is skipped. Select **CI required** as a required status check in
the repository's branch rules to enforce it on merges.

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
npm ci
npm --prefix bindings/javascript ci
npm --prefix packages/emulator-ui ci
npm --prefix apps/web ci
npm --prefix apps/usb-sharing ci
rustup target add wasm32-unknown-unknown
npm --prefix bindings/javascript run setup:tooling

mise run python-lint
mise run python-format-check
mise run python-typecheck
mise run rust-format-check
mise run rust-check
npm --prefix bindings/javascript run build
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
npm --prefix apps/web exec -- playwright install --with-deps chromium webkit
mise run javascript-browser-test
npm --prefix bindings/javascript run test:pyodide:browser
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
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --version 0.9.1 --locked
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
