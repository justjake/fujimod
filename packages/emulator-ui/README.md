# Emulator UI

`@fujimod/emulator-ui` owns the session controller, workspace, LCD viewer,
control renderer, card browser, loading and connection status, faults, Undo,
webcam input, and report UI. The homepage imports these implementations from
this package. The `Engine` contract and worker protocol each have one definition
under `src/engine/`.

The session controller receives an engine factory, boot configuration and
frontend callbacks. It uses only `Engine`; it does not import an engine
implementation, catalog, firmware patcher or saved-camera library. The web app
supplies `WasmEngine` and keeps firmware choice and saved-camera policy in
`apps/web/src/home/controller.ts`.

`EmulatorWorkspace` renders the session's viewer, inputs, card and status. Its
slots hold frontend controls and the firmware prompt. `DevicePanel` remains the
control layout data used by the shared renderer. Module exports point directly
to their implementations; there is no facade or compatibility layer.

`src/style.css` owns shared emulator presentation. The web app's
`src/home/site.css` adds its shell, catalog and page styles. Lucide icon
attribution remains in [the web app's license](../../apps/web/third_party/lucide-LICENSE).

From the repository root:

```sh
npm ci
npm --prefix packages/emulator-ui ci
npm --prefix apps/web ci
npm --prefix apps/web run check
npm --prefix apps/web test
```

Running `npm test` in this package runs its unit and Solid component tests. The
web app's `npm test` runs this package's tests and then the app tests. Each
package's Vite configuration discovers tests only under its own source tree.
Browser integration checks remain in `apps/web/`, alongside the frontend they
exercise.
