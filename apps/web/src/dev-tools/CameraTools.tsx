import { Show, createSignal } from 'solid-js';
import type { Camera, CameraModel, ToolSession } from './camera-model';
import type { AddFile } from '../files/shelf';
import { Wizard } from './routes';

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** The tools for the selected camera: Read, Console and Write. */
export function CameraTools(props: {
  camera: Camera;
  model: CameraModel;
  onFile: AddFile;
  onWizard: (wizard: Wizard) => void;
}) {
  const [command, setCommand] = createSignal('');
  const [running, setRunning] = createSignal('');
  const [status, setStatus] = createSignal('');
  const available = () =>
    props.camera.present && !props.camera.uncertain && !props.camera.session && !running();
  /** Runs `operation` in a session held by `tool`. Reads the camera once, when the action starts. */
  async function run(
    tool: string,
    operation: (session: ToolSession, file: AddFile) => Promise<void>,
  ) {
    if (!available()) return;
    const id = props.camera.id;
    const model = props.model;
    const file = props.onFile;
    setRunning(tool);
    setStatus('');
    try {
      await model.withSession(id, tool, (session) => operation(session, file));
    } catch (error) {
      setStatus(message(error));
    } finally {
      setRunning('');
    }
  }
  function dumpConfiguration() {
    void run('Dump configuration', async (session, file) => {
      const { capacity, pageBytes } = await session.configurationGeometry();
      // Host allocation budget, independent of camera identity. Never infer geometry.
      if (
        !Number.isInteger(capacity) ||
        capacity <= 0 ||
        capacity > 64 * 1024 * 1024 ||
        !Number.isInteger(pageBytes) ||
        pageBytes < 1
      )
        throw Error('Cannot verify configuration geometry within the 64 MiB host budget.');
      const data = new Uint8Array(capacity);
      let offset = 0;
      try {
        for (; offset < capacity; offset += pageBytes) {
          const piece = await session.readConfiguration(
            offset,
            Math.min(pageBytes, capacity - offset),
          );
          data.set(piece, offset);
        }
      } finally {
        const complete = offset >= capacity;
        if (offset > 0)
          file('configuration.bin', new Blob([data.slice(0, Math.min(offset, capacity))]), {
            capacity,
            pageBytes,
            complete,
          });
      }
    });
  }
  function sendCommand(event: SubmitEvent) {
    event.preventDefault();
    const text = command().trim();
    if (!text) return;
    void run('Console command', async (session) => {
      await session.submitConsole(new TextEncoder().encode(`${text}\n`));
      // Output appears in the log as it arrives; stop waiting after a bounded number of polls.
      for (let poll = 0; poll < 600; poll++) if ((await session.pollConsole()).complete) return;
      throw Error('The command is still running; its output continues in the log.');
    });
  }
  return (
    <div class="wb-tools-body">
      <section class="wb-section" aria-labelledby="wb-read">
        <h3 id="wb-read" class="wb-label">
          Read
        </h3>
        <button class="wb-btn" disabled={!available()} onClick={dumpConfiguration}>
          {running() === 'Dump configuration' ? 'Dumping configuration…' : 'Dump configuration'}
        </button>
        <button class="wb-btn" onClick={() => props.onWizard(Wizard.MapControls)}>
          Capture buttons
        </button>
      </section>

      <form class="wb-command" onSubmit={sendCommand}>
        <input
          class="wb-command-input"
          aria-label="Console command"
          placeholder="Console command"
          value={command()}
          onInput={(event) => setCommand(event.currentTarget.value)}
          spellcheck={false}
          autocomplete="off"
          autocapitalize="off"
          enterkeyhint="send"
        />
        <button class="wb-btn" type="submit" disabled={!available() || !command().trim()}>
          Send
        </button>
      </form>

      <section class="wb-section" aria-labelledby="wb-write">
        <h3 id="wb-write" class="wb-label wb-write">
          Write
        </h3>
        <div class="wb-tool wb-write">
          <div class="wb-tool-head">
            Debug mode<span class="wb-writes">Changes a camera setting</span>
          </div>
          <p class="wb-explain">
            In debug mode, the camera operates normally, except its USB connection can be used to
            inspect and control the camera’s internal software. USB must be plugged in twice for
            regular file transfer.
          </p>
          <div class="wb-debug-state">
            Debug mode:{' '}
            <strong>
              {props.camera.debugMode === 'on'
                ? 'On'
                : props.camera.debugMode === 'off'
                  ? 'Off'
                  : props.camera.debugMode === 'unknown'
                    ? 'Could not be read'
                    : 'Reading…'}
            </strong>
          </div>
          <div class="wb-pair wb-single">
            <button
              class="wb-btn"
              disabled={
                !available() || props.camera.transport !== 'usb' || props.camera.debugMode === 'off'
              }
              onClick={() => props.onWizard(Wizard.Disable)}
            >
              Disable debug mode
            </button>
          </div>
        </div>
      </section>

      <Show when={status()}>
        <p class="wb-status" role="status">
          {status()}
        </p>
      </Show>
    </div>
  );
}
