import { PowerComboKeys } from '@fujimod/emulator-ui/ui/power-combo';
import { For, Match, Show, Switch, createSignal } from 'solid-js';
import { serviceModeControls, serviceModeEntry } from './service-mode-entry';
import './debug-mode/wizard.css';

/** What the guide needs to know about the selected camera's connection. */
export enum GuideConnection {
  Unsupported = 'unsupported',
  Disconnected = 'disconnected',
  Choosing = 'choosing',
  Connected = 'connected',
}
enum Step {
  Overview,
  ServiceMode,
  Connect,
}
const rail = [
  [Step.ServiceMode, 'Service mode'],
  [Step.Connect, 'Connect'],
] as const;

/** How to enter service mode and connect. It never changes a setting. */
export function ServiceModeGuide(props: {
  connection: GuideConnection;
  onConnect: () => void;
  onClose: () => void;
}) {
  const [step, setStep] = createSignal(Step.Overview);
  const [ready, setReady] = createSignal(false);
  const connection = () => props.connection;
  function allowAccess() {
    setStep(Step.Connect);
    props.onConnect();
  }
  return (
    <section class="dm" aria-labelledby="dm-title">
      <div class="dm-top">
        <button class="dm-back" onClick={() => props.onClose()}>
          ← Tools
        </button>
        <Show when={step() !== Step.Overview}>
          <ol class="dm-rail" aria-label="Steps">
            <For each={rail}>
              {([id, label], index) => (
                <li
                  class={{ done: step() > id, current: step() === id }}
                  aria-current={step() === id ? 'step' : undefined}
                >
                  <span class="dm-rail-number">{String(index() + 1).padStart(2, '0')}</span>
                  <span class="dm-rail-label">{label}</span>
                </li>
              )}
            </For>
          </ol>
        </Show>
      </div>
      <Switch>
        <Match when={step() === Step.Overview}>
          <div class="dm-panel">
            <h1 id="dm-title" class="dm-title">
              Enter service mode
            </h1>
            <p class="dm-lead">Service mode exposes the camera’s ff80 USB connection.</p>
            <p class="dm-caution">
              Service mode restores factory settings when the camera is switched off. Removing the
              battery avoids that. You need a USB cable and about five minutes.
            </p>
            <button class="dm-button dm-next" onClick={() => setStep(Step.ServiceMode)}>
              Begin
            </button>
          </div>
        </Match>
        <Match when={step() === Step.ServiceMode}>
          <div class="dm-panel">
            <h1 id="dm-title" class="dm-title">
              Restart into service mode
            </h1>
            <ol class="dm-entry">
              <li>{serviceModeEntry[0]}</li>
              <li>
                <PowerComboKeys controls={serviceModeControls} modes={['service']} />
              </li>
              <li>{serviceModeEntry[1]}</li>
            </ol>
            <div class="dm-warning" role="note">
              <strong>In service mode, do not use the OFF switch.</strong>
              Switching off in service mode restores factory settings. To restart, remove the
              battery.
            </div>
            <label class="dm-check">
              <input
                type="checkbox"
                checked={ready()}
                onChange={(event) => setReady(event.currentTarget.checked)}
              />
              <span>I’m in service mode, the green lamp is on, and USB is connected.</span>
            </label>
            <div class="dm-actions">
              <button class="dm-button dm-secondary" onClick={() => setStep(Step.Overview)}>
                Back
              </button>
              <button
                class={{ 'dm-button': true, 'dm-next': ready() }}
                disabled={!ready()}
                onClick={allowAccess}
              >
                Allow camera access
              </button>
            </div>
          </div>
        </Match>
        <Match when={step() === Step.Connect}>
          <div class="dm-panel">
            <Show when={connection() === GuideConnection.Choosing}>
              <div class="dm-pointer" aria-hidden="true">
                ↑
              </div>
            </Show>
            <h1 id="dm-title" class="dm-title">
              Allow camera access
            </h1>
            <Switch>
              <Match when={connection() === GuideConnection.Connected}>
                <p class="dm-lead dm-ok">Camera connected.</p>
                <button class="dm-button dm-next" onClick={() => props.onClose()}>
                  Open workbench
                </button>
              </Match>
              <Match when={connection() === GuideConnection.Choosing}>
                <p class="dm-lead">
                  In the browser’s dialog at the top, choose the unknown FUJIFILM device, then press
                  Connect.
                </p>
                <p class="dm-caution">
                  No devices listed? The camera isn’t in service mode yet. Cancel, go back and
                  restart it.
                </p>
              </Match>
              <Match when={connection() === GuideConnection.Unsupported}>
                <p class="dm-lead">
                  This browser can’t connect to USB devices. Use Chrome or Edge.
                </p>
              </Match>
              <Match when={true}>
                <p class="dm-lead">The camera isn’t connected.</p>
                <div class="dm-actions">
                  <button class="dm-button dm-secondary" onClick={() => setStep(Step.ServiceMode)}>
                    Back
                  </button>
                  <button class="dm-button dm-next" onClick={allowAccess}>
                    Try again
                  </button>
                </div>
              </Match>
            </Switch>
          </div>
        </Match>
      </Switch>
    </section>
  );
}
