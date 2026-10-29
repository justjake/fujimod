import { BatteryCycleIcon } from '../ui/battery-cycle';
import { Show, createSignal, untrack } from 'solid-js';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import type { JSX } from '@solidjs/web';
import type {
  Camera,
  CameraModel,
  FlashBackup,
  ServicePreparation,
  StateRegion,
  ToolSession,
} from './camera-model';
import { executionSelection, bulkSupportError } from './captures/capture';
import { slug, type AddFile } from '../files/shelf';
import { backupChoice, rememberBackup } from './backup-unlock';
import type { DebugModeOperations } from './debug-mode/operations';
import { Wizard } from './routes';
import { BulkSupportError } from './BulkSupportError';
import { catalog } from 'fujimod';
import { generationFailure } from './generation';
import './getting-started.css';

/** What the path produced on one camera during this page's life. */
export interface PathResults {
  /** The first preparation on this camera: the guard and debug values found before. */
  prepared?: ServicePreparation;
  backup?: { flash: FlashBackup } | 'skipped';
  state?: { regions: StateRegion[] };
  debugSaved?: boolean;
}

type Step = 'todo' | 'running' | 'done' | 'skipped' | 'locked';

function hex(value: number) {
  return `0x${value.toString(16).padStart(8, '0')}`;
}
function mib(bytes: number) {
  return `${(bytes / (1 << 20)).toFixed(bytes < 1 << 20 ? 2 : 0)} MiB`;
}

/** Dev Tools' numbered path: back up flash, dump state, enable debug mode, restart, capture states. */
export function GettingStarted(props: {
  camera: Camera;
  model: CameraModel;
  results: PathResults;
  onResults: (update: (results: PathResults) => PathResults) => void;
  onFile: AddFile;
  debug?: DebugModeOperations;
  onWizard: (wizard: Wizard) => void;
}) {
  const [running, setRunning] = createSignal<'backup' | 'state' | 'debug'>();
  const [progress, setProgress] = createSignal('');
  const [failure, setFailure] = createSignal('');
  const selection = () => executionSelection(props.camera.identity);
  const firmwareId = () => selection()?.firmware.id;
  const bulkError = () => {
    if (!props.camera.present || firmwareId()) return undefined;
    const report = catalog
      .generationResults()
      .find(
        (item) =>
          item.product === props.camera.identity?.product?.trim() &&
          item.internal_version === props.camera.identity?.internalVersion?.trim(),
      );
    return report?.tables.bulk?.status === 'available'
      ? generationFailure(report.firmware_id, 'service')
      : bulkSupportError(props.camera.identity);
  };
  const available = () =>
    props.camera.present &&
    !props.camera.uncertain &&
    !props.camera.session &&
    !running() &&
    Boolean(firmwareId());
  const unlocked = () =>
    props.results.backup !== undefined || backupChoice(props.camera.identity?.serial) !== undefined;
  const restarted = () => props.camera.debugMode === 'on' && !props.camera.serviceMode;

  /** Prepare the service session (idempotent) and run `operation` in the same session. */
  async function step<T>(
    kind: 'backup' | 'state',
    tool: string,
    operation: (session: ToolSession, firmware: string) => Promise<T>,
  ) {
    const firmware = firmwareId();
    const record = props.onResults;
    if (!firmware || !available()) return;
    setRunning(kind);
    setFailure('');
    setProgress('Preparing the service session');
    try {
      return await props.model.withSession(props.camera.id, tool, async (session) => {
        const prepared = await session.prepareServiceSession(firmware);
        record((results) => ({
          ...results,
          prepared: results.prepared ?? prepared,
        }));
        setProgress(kind === 'backup' ? 'Backing up flash' : 'Dumping memory');
        return operation(session, firmware);
      });
    } catch (error) {
      setFailure(shownError(error));
    } finally {
      setRunning(undefined);
      setProgress('');
    }
  }

  async function backUpFlash() {
    const pieces: { offset: number; data: Uint8Array<ArrayBuffer> }[] = [];
    let read = 0;
    const flash = await step('backup', 'Back up flash', (session, firmware) =>
      session.backupFlash(
        firmware,
        ({ offset, data }) => {
          pieces.push({ offset, data: new Uint8Array(data) });
          read += data.length;
          setProgress(`Backing up flash · ${mib(read)}`);
        },
        (done, total) => setProgress(`Backing up flash · ${mib(done)} of ${mib(total)}`),
      ),
    );
    if (!flash) return;
    pieces.sort((a, b) => a.offset - b.offset);
    let end = 0;
    for (const piece of pieces) {
      if (piece.offset !== end) {
        setFailure(`The flash backup has a gap at ${hex(end)}.`);
        return;
      }
      end += piece.data.length;
    }
    if (end !== flash.size) {
      setFailure(`The flash backup covers ${mib(end)} of ${mib(flash.size)}.`);
      return;
    }
    props.onFile('flash.bin', new Blob(pieces.map((piece) => piece.data)), {
      firmware: firmwareId(),
      flash,
    });
    props.onResults((results) => ({ ...results, backup: { flash } }));
    rememberBackup(props.camera.identity?.serial, 'done');
  }

  function skipBackup() {
    props.onResults((results) => ({
      ...results,
      backup: results.backup ?? 'skipped',
    }));
    rememberBackup(props.camera.identity?.serial, 'skipped');
  }

  async function dumpState() {
    const regions: StateRegion[] = [];
    const file = props.onFile;
    const done = await step('state', 'Dump memory', async (session, firmware) => {
      const prepared = untrack(() => props.results.prepared);
      await session.dumpState(
        firmware,
        ({ data, ...region }) => {
          regions.push(region);
          file(`memory-${slug(region.name)}.bin`, new Blob([new Uint8Array(data)]), {
            firmware,
            ...region,
            prepared,
          });
        },
        (done, total) => setProgress(`Dumping memory · ${mib(done)} of ${mib(total)}`),
      );
      return true;
    });
    if (!done) return;
    props.onResults((results) => ({ ...results, state: { regions } }));
  }

  async function enableDebugMode() {
    const firmware = firmwareId();
    if (!firmware || !props.debug || !available() || !unlocked()) return;
    setRunning('debug');
    setFailure('');
    try {
      await props.debug.enter(firmware, setProgress);
      props.onResults((results) => ({ ...results, debugSaved: true }));
    } catch (error) {
      setFailure(shownError(error));
    } finally {
      setRunning(undefined);
      setProgress('');
    }
  }

  const backupStep = (): Step =>
    running() === 'backup'
      ? 'running'
      : props.results.backup === 'skipped'
        ? 'skipped'
        : props.results.backup
          ? 'done'
          : 'todo';
  const debugDisabled = () => !available() || !unlocked() || !props.debug || debugStep() === 'done';
  const debugStep = (): Step =>
    running() === 'debug'
      ? 'running'
      : props.results.debugSaved || restarted()
        ? 'done'
        : unlocked()
          ? 'todo'
          : 'locked';

  return (
    <section class="gs" aria-labelledby="gs-title">
      <h3 id="gs-title" class="wb-label">
        Get started
      </h3>
      <ol class="gs-steps">
        <Item number={1} state={backupStep()}>
          <div class="gs-stack">
            <div class="gs-row">
              <button
                class="wb-btn wb-primary"
                disabled={!available() || backupStep() === 'done'}
                onClick={() => void backUpFlash()}
              >
                Back up flash
              </button>
              <Show when={!props.results.backup}>
                <button class="wb-text" disabled={Boolean(running())} onClick={skipBackup}>
                  Skip
                </button>
              </Show>
              <Show when={backupStep() === 'skipped'}>
                <span class="gs-tag">Skipped</span>
              </Show>
            </div>
            <BulkSupportError reason={bulkError()} />
          </div>
        </Item>
        <Item
          number={2}
          state={running() === 'state' ? 'running' : props.results.state ? 'done' : 'todo'}
        >
          <div class="gs-stack">
            <button class="wb-btn" disabled={!available()} onClick={() => void dumpState()}>
              Dump memory
            </button>
            <BulkSupportError reason={bulkError()} />
          </div>
        </Item>
        <Item number={3} state={debugStep()}>
          <div class="gs-stack">
            <button
              class="wb-btn"
              disabled={debugDisabled()}
              onClick={() => void enableDebugMode()}
            >
              Enable debug mode
            </button>
            <BulkSupportError reason={bulkError()} />
            <p class={{ 'gs-note': true, 'gs-disabled': debugDisabled() }}>
              In debug mode your camera works normally, but its USB connection can also inspect and
              control the camera’s software. Ordinary file transfer needs USB plugged in twice.{' '}
              <span class="gs-writes">
                It changes a camera setting that stays on, even after restarts, until you turn it
                off.
              </span>
            </p>
          </div>
        </Item>
        <Item number={4} state={restarted() ? 'done' : 'todo'}>
          <p class="gs-note">
            <BatteryCycleIcon />
            Remove and reinsert the battery, then switch on and reconnect.
          </p>
        </Item>
        <Item number={5} state="todo">
          <button
            class="wb-btn"
            disabled={!restarted()}
            onClick={() => props.onWizard(Wizard.StateCaptures)}
          >
            Capture camera modes
          </button>
        </Item>
      </ol>
      <Show when={progress()}>
        <p class="wb-status" role="status">
          {progress()}
        </p>
      </Show>
      <Show when={failure()}>
        <p class="wb-status gs-failure" role="alert">
          {failure()}
        </p>
      </Show>
    </section>
  );
}

function Item(props: { number: number; state: Step; children: JSX.Element }) {
  return (
    <li class="gs-step" data-state={props.state}>
      <span class="gs-number" aria-hidden="true">
        {props.state === 'done' ? <Icon name="check" size={12} /> : props.number}
      </span>
      <div class="gs-row">{props.children}</div>
    </li>
  );
}
