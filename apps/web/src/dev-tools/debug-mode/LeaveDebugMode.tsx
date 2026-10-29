import { BatteryCycleIcon } from '../../ui/battery-cycle';
import { For, Show, createSignal } from 'solid-js';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import './wizard.css';

/** Persist debug access disabled through the connected USB session. */
export interface LeaveDebugModeOperation {
  leave(progress: (line: string) => void): Promise<void>;
}
export function LeaveDebugMode(props: {
  connected: boolean;
  operation?: LeaveDebugModeOperation;
  onClose: () => void;
}) {
  const [lines, setLines] = createSignal<string[]>([]);
  const [failure, setFailure] = createSignal('');
  const [running, setRunning] = createSignal(false);
  const [done, setDone] = createSignal(false);
  async function leave() {
    if (!props.operation || !props.connected || running()) return;
    setRunning(true);
    setFailure('');
    setLines([]);
    try {
      await props.operation.leave((line) => setLines((previous) => [...previous, line]));
      setDone(true);
    } catch (error) {
      setFailure(shownError(error));
    } finally {
      setRunning(false);
    }
  }
  return (
    <section class="dm" aria-labelledby="dm-leave-title">
      <div class="dm-top">
        <button class="dm-back" onClick={() => props.onClose()}>
          ← Tools
        </button>
      </div>
      <div class="dm-panel">
        <h1 id="dm-leave-title" class="dm-title">
          Leave debug mode
        </h1>
        <p class="dm-lead">
          Turns the Debug Mode setting off. After the next restart, USB works for ordinary file
          transfer the first time it is plugged in.
        </p>
        <Show
          when={!done()}
          fallback={
            <p class="dm-lead dm-ok">
              Debug mode is off. <BatteryCycleIcon />
              Remove and reinsert the battery, then switch the camera on.
            </p>
          }
        >
          <button
            class={{
              'dm-button': true,
              'dm-huge': true,
              'dm-next': Boolean(props.operation) && props.connected && !running(),
            }}
            disabled={!props.operation || !props.connected || running()}
            onClick={() => void leave()}
          >
            Leave debug mode
          </button>
        </Show>
        <Show when={lines().length}>
          <pre class="dm-log" aria-live="polite">
            <For each={lines()}>{(line) => <span>{`> ${line}\n`}</span>}</For>
          </pre>
        </Show>
        <Show when={failure()}>
          <p class="dm-error" role="alert">
            {failure()}
          </p>
        </Show>
      </div>
    </section>
  );
}
