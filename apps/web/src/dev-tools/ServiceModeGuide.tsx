import { BatteryCycleIcon } from '../ui/battery-cycle';
import { PowerComboKeys } from '@fujimod/emulator-ui/ui/power-combo';
import { For, Match, Show, Switch, createEffect, createSignal } from 'solid-js';
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
  const [showConnectionHelp, setShowConnectionHelp] = createSignal(false);
  const connection = () => props.connection;
  createEffect(
    () => connection() === GuideConnection.Choosing,
    (choosing) => {
      if (!choosing) return;
      setShowConnectionHelp(false);
      const timer = setTimeout(() => setShowConnectionHelp(true), 3000);
      return () => clearTimeout(timer);
    },
  );
  createEffect(
    () => ({ step: step(), connection: connection() }),
    (state) => {
      if (state.step === Step.ServiceMode && state.connection === GuideConnection.Connected)
        setStep(Step.Connect);
      else if (state.step === Step.Connect && state.connection !== GuideConnection.Connected)
        setStep(Step.ServiceMode);
    },
  );
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
              <li>
                <span>
                  <BatteryCycleIcon />
                  {serviceModeEntry[0]}
                </span>
              </li>
              <li>
                <PowerComboKeys controls={serviceModeControls} modes={['service']} />
              </li>
              <li>{serviceModeEntry[1]}</li>
            </ol>
            <div class="dm-warning" role="note">
              <strong>In service mode, do not use the OFF switch.</strong>
              <span>
                Switching off in service mode restores factory settings. <BatteryCycleIcon />
                To restart, remove and reinsert the battery.
              </span>
            </div>
            <div class="dm-actions">
              <button
                class="dm-button dm-secondary"
                disabled={connection() === GuideConnection.Choosing}
                onClick={() => setStep(Step.Overview)}
              >
                Back
              </button>
              <button
                class="dm-button dm-next"
                disabled={
                  connection() === GuideConnection.Choosing ||
                  connection() === GuideConnection.Unsupported
                }
                onClick={() => props.onConnect()}
              >
                Try to Connect
              </button>
            </div>
            <Show when={connection() === GuideConnection.Choosing}>
              <p class="dm-caution" role="status">
                Choose your camera in the browser’s dialog, then select Connect.
              </p>
            </Show>
            <Show when={showConnectionHelp()}>
              <p class="dm-caution" role="status">
                Can’t see your camera? It must not be in service mode. Our instructions might be
                wrong, or you might need to try a few times.
              </p>
            </Show>
            <Show when={connection() === GuideConnection.Unsupported}>
              <p class="dm-caution">
                This browser can’t connect to USB devices. Use Chrome or Edge.
              </p>
            </Show>
          </div>
        </Match>
        <Match when={step() === Step.Connect}>
          <div class="dm-panel">
            <h1 id="dm-title" class="dm-title">
              Camera connected
            </h1>
            <button class="dm-button dm-next" onClick={() => props.onClose()}>
              Open workbench
            </button>
          </div>
        </Match>
      </Switch>
    </section>
  );
}
