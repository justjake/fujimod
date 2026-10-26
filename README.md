# FujiMod

Camera tools, emulation and firmware mods. The browser application lives in
[apps/web](apps/web/README.md), including its body definitions, patch
interpreter, runtime adapter and browser checks. The shared
[emulator UI](packages/emulator-ui/README.md) lives in `packages/emulator-ui/`.

Use `mise install`, then work in `apps/web`:

```sh
npm ci
npm --prefix packages/emulator-ui ci
cd apps/web
npm ci
npm test
npm run dev
```

The root `web-build`, `web-test` and `web-preview` mise tasks run from that
application directory. `web-test` runs both `@fujimod/emulator-ui` tests and the
web app tests through `apps/web`'s `npm test`. The Pages workflow builds
`apps/web` from approved main releases. Review branches do not deploy Pages.

## Rust libraries

The root Cargo workspace contains reusable implementation crates under `crates/`.
The `fujimod` facade exposes components through optional features; enable `usb`
for `fujimod::usb`. The [mod installation crate](crates/mod-install/README.md)
provides checked uploads and borrowed callback batches. The [USB crate](crates/usb/README.md) provides the initial
asynchronous service/debug protocol implementation.
The [catalog crates](crates/catalog/README.md) load caller-selected firmware
data: the reference catalog through `fujimod::catalog`, and the service
definitions USB uses through `fujimod::catalog_service`.
Enable the `emulator` feature for `fujimod::emulator`; execution, device models
and processor backends live in separate crates under `crates/emulator*`.
The Python package exposes the same execution core through `fujimod.emulator`.

External EXR II board composers can consume `fujimod_emulator_exr2_boot_serial::BootSerialBlocks::devices()` into their device graph. Register each `Some(Mapping)` window and attach each `None` entry as an unmapped event device; the returned devices share one serial state and coordinator.

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Every member depends on `fujimod-workspace-hack`, which
[cargo-hakari](https://docs.rs/cargo-hakari) generates so that any package
selection builds third-party crates with the same features and reuses one
compiled copy. After adding or changing a dependency, run `mise run rust-hack`
(install the tool with `cargo install cargo-hakari --version 0.9.39 --locked`);
CI fails when the crate is out of date. `fujimod-unicorn-sys` compiles the
Unicorn C archive with no features, so every feature set, test and binary of
the CPU backend shares one archive per profile.

Development uses two profiles: `cargo test` builds the default debug profile,
and every optimized local build (`--release`, `scripts/python_extension.py`,
the emulator worker) shares `release`, which rebuilds in seconds: FujiMod
crates at opt-level 2 without LTO, incremental, with dependencies fully
optimized. `--profile dist` adds ThinLTO for published wheels and the web
emulator; it runs about 5% faster and links for about 40 seconds longer.

The core also builds for WebUSB. See the USB crate's
[Wasm checks](crates/usb/README.md#wasm-core-checks) for the JS-runtime tests;
the [JavaScript package](bindings/javascript/README.md) provides typed browser
APIs over the same core. Python in Pyodide uses the
[WebUSB adapter](bindings/python/PYODIDE.md) with that initialized module.

The [Python package](bindings/python/README.md) lives in `bindings/python/`.

Rust owns each browser USB device for the page's lifetime. Local sessions and
remote controllers are both clients of that one nusb owner, with one session
ownership lock. The browser end runs the Rust/Wasm exporter; JavaScript only
routes its network messages. Sharing attaches or detaches peers independently
of the physical connection. See [USB sharing](crates/usb-proxy/SHARING.md).

The JavaScript/Wasm package lives in `bindings/javascript/`. Binding crates are
members of this same workspace. Each directory owns its language package;
implementation crates remain independent of PyO3 and wasm-bindgen. The browser
application stays in `apps/web/` and consumes the JavaScript package as needed.

The [shared binding generator](bindings/generator/README.md) derives service
value wrappers, command/action factories, structured replies and language exports
from Rust declarations. Routine service features need only Rust code; shared
adapters handle execution and language boundaries.

## Continuous integration

See [CI.md](CI.md) for the required checks, local commands, coverage reports and
the distinction between public tests and firmware-dependent rehearsals.
