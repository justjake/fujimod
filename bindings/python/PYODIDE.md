# Python over WebUSB

Pyodide uses `fujimod.usb` with the same awaitable commands, enums, byte results
and exception categories as native Python. A thin Python adapter calls the
standalone JavaScript binding; that binding executes the Rust protocol core
and nusb WebUSB. The Python and USB Wasm modules have separate memories. Buffers
are copied across that boundary before I/O; no Python-memory view survives a
request. This adapter contains no USB framing or firmware policy.

The browser-only wheel omits `fujimod.emulator`, which is available in native
Python wheels.

Build the two artifacts from the repository root:

```sh
npm ci
npm --prefix bindings/javascript ci
npm --prefix bindings/javascript run build
node bindings/python/scripts/build-web-wheel.ts
```

The wheel builder uses the pinned Node toolchain. It creates the `fujimod` distribution
with the `py3-none-pyemscripten_2026_0_wasm32` tag in `target/web-wheels/`, for
Pyodide 314.x. It does not ship a desktop binary or claim desktop compatibility.
Native Python uses the same package with the extension that
`scripts/python_extension.py` builds. Keep the Python wheel and JavaScript
package from the same version together.

After loading Pyodide, initialize and register the JavaScript package:

```javascript
import { initialize, registerPyodide } from 'fujimod';
await initialize();
registerPyodide(pyodide);
await pyodide.loadPackage('micropip');
await pyodide.runPythonAsync(`
import micropip
await micropip.install('/assets/fujimod-0.1.0-py3-none-pyemscripten_2026_0_wasm32.whl')
from fujimod import usb
`);
```

Serve the wheel and JavaScript/Wasm assets over HTTPS or localhost. The browser
controls WebUSB availability and permissions. `await usb.discover(filter)` lists
already permitted devices. Invoke `await usb.request_device(filter)` directly
from a user gesture to show the browser picker; callers still choose an explicit
interface and supply firmware limits. WebUSB does not provide OS bus IDs, port
paths or device addresses, so those metadata properties are `None`.

The adapter supports async commands, compositions, owned chunk iterators and
bounded trace snapshots. Use `async with` around a reader when stopping early.
Canceling a command or pending chunk requests host cancellation in Rust and
awaits its outcome before returning ownership. An incomplete exchange leaves
`CompletionUnknown`; it does not send C4 or recovery. Canceling an open waits
for the open outcome, then abandons a session if one was created. Explicit
abandonment remains terminal and cannot stop camera execution.

Run the end-to-end checks after building both artifacts:

```sh
npm --prefix bindings/javascript run test:pyodide
npx --prefix bindings/javascript playwright install chromium
npm --prefix bindings/javascript run test:pyodide:browser
```

The Node check caches the runtime's micropip wheel under
`target/pyodide-test-packages/`; the browser check serves that cached wheel.
Both install the actual FujiMod wheel with micropip and run Python through
compiled Rust and the real nusb backend against fake WebUSB devices. Tests
cover buffers, enums, uploads, iterator ownership, structured failures, tracing,
timeouts and cancellation. No test contacts hardware or validates permission UI.

The wheel also exposes shared firmware definitions as `fujimod.catalog` and typed
service controls as `fujimod.usb.app80`, using the
[same Python interface](README.md#firmware-selected-service-controls) as native
builds. `registerPyodide` registers both namespaces. JSON validation, ordered
command selection, encoding, reply interpretation and I/O all execute in Rust.
The Python adapter supplies frozen payload-enum variants and copied buffers.
Definition and command-selection exceptions retain typed `.reason` values.

The end-to-end contract exercises every supported service command/result variant,
nondefault command codes and argument layouts, unreviewed definitions, explicit
identity mismatches and unknown result bytes. It then executes a typed service
request through nusb/WebUSB, checks a malformed nested reply, and cancels during
publication. Both failures leave session completion unknown and issue no close.

## Same-memory extension experiment

The native PyO3 extension does not compile unchanged for WebUSB: its async
coroutines require `Send`, while browser USB objects are confined to one thread.
With PyO3 0.30.1, wasm-bindgen 0.2.129, Rust nightly-2026-10-10 and Emscripten
5.0.3, a minimal PyO3 side module calling `js_sys::Array::of1` builds. Loading the
raw module in Pyodide 314.0.7 fails on unresolved wasm-bindgen imports. Processing
it emits `library_bindgen.js` for main-module integration; loading only the
transformed side module fails at the call with a function-table error.

[Pyodide's Python 3.14 ABI](https://pyodide.org/en/stable/development/abi/314.html)
uses Emscripten 5.0.3. The current
[wasm-bindgen Emscripten integration](https://wasm-bindgen.github.io/wasm-bindgen/reference/emscripten.html)
describes newer main-module builds. Those facilities do not establish support
for this side-module arrangement in stock Pyodide. A same-memory binding would
need additional runtime integration; this result does not establish that such
an integration is impossible.
