import { Show, For, createSignal, onSettled, untrack } from 'solid-js';
import DevTools from '../dev-tools/Page';
import type { CameraModel } from '../dev-tools/camera-model';
import { downloadBlob } from '@fujimod/emulator-ui/ui/download';
import { FileList } from '@fujimod/emulator-ui/ui/file-list';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import { ContributeDialog } from '../files/ContributeDialog';
import { uploadReport, type UploadReport, type UploadReceipt } from './upload';
import { reportPlatform, type ReportPlatform } from './file-guide';
import {
  parseReportLink,
  decode64,
  validateReport,
  reportName,
  reportEntries,
  type ReportEntry,
  type ReportLink,
} from './report-link';
import { prepareDownload } from './download';
import { ReportUsb, type CameraChoice, type ReportConnection } from './usb';
import '../home/site.css';
import './style.css';
export default function BugReport(props: {
  connection?: ReportConnection;
  hash?: string;
  supported?: boolean;
  upload?: UploadReport;
  /** The Dev Tools camera model to open; the page's own by default. */
  devToolsModel?: CameraModel;
}) {
  const connection = untrack(() => props.connection) ?? new ReportUsb();
  const supported =
    untrack(() => props.supported) ??
    (typeof window !== 'undefined' && isSecureContext && 'usb' in navigator);
  const [link, setLink] = createSignal<ReportLink>();
  const [archive, setArchive] = createSignal<Blob>();
  const [download, setDownload] = createSignal<{ blob: Blob; name: string }>();
  const [choice, setChoice] = createSignal<CameraChoice>();
  const [selected, setSelected] = createSignal(0);
  const [status, setStatus] = createSignal(''),
    [error, setError] = createSignal('');
  const [busy, setBusy] = createSignal(false),
    [ready, setReady] = createSignal(false);
  const [connected, setConnected] = createSignal(false),
    [devTools, setDevTools] = createSignal(false);
  const upload = untrack(() => props.upload) ?? uploadReport;
  const [platform, setPlatform] = createSignal<ReportPlatform>();
  const [entries, setEntries] = createSignal<ReportEntry[]>([]);
  const [fromUsb, setFromUsb] = createSignal(false);
  const [remove, setRemove] = createSignal(true);
  const [receipt, setReceipt] = createSignal<UploadReceipt>();
  let uploadId = '';
  const [contributing, setContributing] = createSignal(false);
  let handedOff = false;
  onSettled(() => {
    setPlatform(reportPlatform(navigator.userAgent, navigator.maxTouchPoints));
    try {
      const value = parseReportLink(untrack(() => props.hash) ?? location.hash);
      setLink(value);
      if (value.archive) {
        void acceptArchive(validateReport(decode64(value.archive), value), value)
          .then(() => setStatus('Report decoded from QR code.'))
          .catch(problem);
      } else if (supported)
        void connection
          .initialize()
          .then(() => setReady(true))
          .catch(problem);
    } catch (cause) {
      problem(cause);
    }
    const stop = connection.onDisconnect?.(() => {
      setConnected(false);
      setStatus('Camera disconnected. Your report remains available.');
    });
    return () => {
      stop?.();
      if (!handedOff) connection.dispose();
    };
  });
  function problem(cause: unknown) {
    setError(cause instanceof Error ? cause.message : String(cause));
    setBusy(false);
  }
  async function acceptArchive(blob: Blob, current: ReportLink) {
    setEntries(reportEntries(new Uint8Array(await blob.arrayBuffer()), current));
    setArchive(blob);
    setReceipt(undefined);
    uploadId = crypto.randomUUID();
    setDownload(undefined);
    setStatus('Preparing download…');
    setDownload(await prepareDownload(blob, reportName(current)));
  }
  async function read(id: number) {
    setBusy(true);
    setError('');
    setStatus('Reading report…');
    const current = link()!;
    try {
      await acceptArchive(
        await connection.read(id, current, (bytes) =>
          setStatus(`Reading report… ${Math.floor((bytes / current.bytes) * 100)}%`),
        ),
        current,
      );
      setConnected(true);
      setFromUsb(true);
      setStatus('Report retrieved from camera.');
    } catch (cause) {
      problem(cause);
    } finally {
      setBusy(false);
    }
  }
  async function connect() {
    if (busy()) return;
    setBusy(true);
    setError('');
    try {
      const value = await connection.choose();
      setChoice(value);
      if (value?.interfaces.length === 1) await read(value.interfaces[0].id);
      else if (value && !value.interfaces.length) throw Error('No debug interface is available.');
    } catch (cause) {
      problem(cause);
    } finally {
      setBusy(false);
    }
  }
  async function chooseFile(event: Event) {
    const file = (event.currentTarget as HTMLInputElement).files?.[0];
    if (!file || !link()) return;
    setError('');
    setBusy(true);
    try {
      if (file.size !== link()!.bytes) throw Error('Choose the TAR file named in this report.');
      setFromUsb(false);
      await acceptArchive(
        validateReport(new Uint8Array(await file.arrayBuffer()), link()!),
        link()!,
      );
      setFromUsb(false);
      setStatus('Report opened from file.');
    } catch (cause) {
      problem(cause);
    } finally {
      setBusy(false);
    }
  }
  /** The Contribute dialog's upload: the issue report, then deletion from the camera if chosen. */
  async function contribute(message: string, progress: (fraction: number) => void) {
    if (!archive() || !link()) throw Error('No report is loaded.');
    setBusy(true);
    setError('');
    progress(0);
    try {
      const result = await upload(archive()!, link()!, message, uploadId);
      progress(1);
      setReceipt(result);
      const confirmation = result.issueURL
        ? 'Report uploaded and GitHub issue created.'
        : 'Report uploaded. GitHub issue creation is pending.';
      setStatus(confirmation);
      if (fromUsb() && connected() && remove() && connection.remove) {
        try {
          await connection.remove(link()!);
          setStatus(confirmation + ' Deleted from camera.');
        } catch {
          setError('Your report was uploaded, but could not be deleted from the camera.');
        }
      }
      return {
        id: result.id,
        link: result.issueURL ? { href: result.issueURL, label: 'View GitHub issue' } : undefined,
      };
    } finally {
      setBusy(false);
    }
  }
  return (
    <Show
      when={devTools()}
      fallback={
        <main class="shell bug-report">
          <header>
            <a href="../">
              <img src="../brand.png" alt="FujiMod" width="110" height="28" />
            </a>
          </header>
          <Show when={link()}>
            {(value) => (
              <>
                <p>
                  This web app uses WebUSB to read issue reports from a FujiMod camera showing the
                  issue QR code. It also works if USB debugging is enabled.
                </p>
                <p>
                  <strong>
                    When your camera is showing the QR code, it’s ready to upload your error report
                    over USB.
                  </strong>
                </p>
                <div class="bug-paths">
                  <section
                    class={{ 'bug-card': true, 'bug-usb-unavailable': !supported }}
                    aria-label="USB connection"
                  >
                    <Show when={!supported}>
                      <h2 class="bug-unavailable-title">WebUSB unavailable</h2>
                      <p class="bug-unavailable-reason">
                        Use Chrome or Edge on a computer, or choose the TAR file from your SD card.
                      </p>
                    </Show>
                    <Show when={supported}>
                      <h2>Connect over USB</h2>
                      <p>
                        Leave the QR screen open and plug in USB. If it was already plugged in,
                        unplug and reconnect it.
                      </p>
                      <p class="bug-wait">
                        <span class={{ 'bug-spinner': busy() }} aria-hidden="true" />
                        {busy()
                          ? 'Working…'
                          : connected()
                            ? 'Camera connected'
                            : 'Waiting for camera connection'}
                      </p>
                      <button
                        class="pill primary"
                        disabled={!ready() || busy() || connected()}
                        onClick={() => void connect()}
                      >
                        Connect camera
                      </button>
                      <Show when={choice() && choice()!.interfaces.length > 1 && !connected()}>
                        <label>
                          Debug interface
                          <select
                            value={selected()}
                            onChange={(event) => setSelected(Number(event.currentTarget.value))}
                          >
                            <For each={choice()!.interfaces}>
                              {(item) => <option value={item.id}>{item.label}</option>}
                            </For>
                          </select>
                        </label>
                        <button
                          class="pill"
                          disabled={busy()}
                          onClick={() => void read(selected())}
                        >
                          Read report
                        </button>
                      </Show>
                      <details>
                        <summary>After restarting the camera</summary>
                        <p>
                          Open the issue QR screen again, or enable SET UP → FUJIMOD DEVTOOLS → USB
                          DEBUGGING, then reconnect USB.
                        </p>
                      </details>
                    </Show>
                  </section>
                  <section class="bug-card" aria-label="SD card file">
                    <h2>Choose from SD card</h2>
                    <p>
                      Alternatively, find the issue report TAR file on the SD card at{' '}
                      <code class="bug-file">{value().path.slice(2).replaceAll('\\', '/')}</code>
                    </p>
                    <Show when={platform()}>
                      {(device) => (
                        <>
                          <p class="bug-platform">
                            <svg
                              width="32"
                              height="32"
                              viewBox="0 0 32 32"
                              fill="none"
                              stroke="currentColor"
                              stroke-width="1.5"
                              aria-hidden="true"
                            >
                              <Show
                                when={device().mobile}
                                fallback={
                                  <>
                                    <rect x="3" y="5" width="26" height="18" rx="2" />
                                    <path d="M16 23v5m-6 0h12" />
                                  </>
                                }
                              >
                                <rect x="8" y="2" width="16" height="28" rx="3" />
                                <path d="M13 26h6" />
                              </Show>
                            </svg>
                            <span>
                              {device().name} · {device().fileManager}
                            </span>
                          </p>
                          <p>{device().guide}</p>
                        </>
                      )}
                    </Show>
                    <p>
                      If browsing the camera over USB/PTP only shows photos or videos, use a card
                      reader to access this folder.
                    </p>
                    <label class="bug-file-picker">
                      Choose report TAR
                      <input
                        type="file"
                        accept=".tar,application/x-tar"
                        disabled={busy()}
                        onChange={(event) => void chooseFile(event)}
                      />
                    </label>
                  </section>
                </div>
                <section class="bug-card bug-contents">
                  <p>
                    Your camera data is private and remains on your device until you press upload.
                    Uploaded issue reports contain this data:
                  </p>
                  <Show when={archive()}>
                    <h2>Report ready</h2>
                  </Show>
                  <Show
                    when={archive()}
                    fallback={
                      <ul>
                        <li>Current screen image</li>
                        <li>Recent FujiMod activity and stock logs</li>
                        <li>Camera UI state, settings and task diagnostics</li>
                        <li>Firmware and FujiMod build information</li>
                      </ul>
                    }
                  >
                    <p class="bug-file">
                      {reportName(value())} · {archive()!.size.toLocaleString()} bytes
                    </p>
                    <FileList
                      items={entries().map((entry) => ({
                        key: entry.name,
                        name: entry.name,
                        bytes: entry.bytes,
                      }))}
                      download={(item) => {
                        const entry = entries().find((candidate) => candidate.name === item.key);
                        if (entry)
                          downloadBlob(
                            archive()!.slice(entry.offset, entry.offset + entry.bytes),
                            entry.name.split('/').at(-1)!,
                          );
                      }}
                    />
                  </Show>
                  <div class="bug-actions">
                    <button
                      class="pill"
                      type="button"
                      disabled={!download()}
                      onClick={() => downloadBlob(download()!.blob, download()!.name)}
                    >
                      Download all
                    </button>
                    <button
                      class="pill primary"
                      type="button"
                      disabled={!archive() || busy() || !!receipt()}
                      onClick={() => setContributing(true)}
                    >
                      <Icon name="upload" size={14} />
                      Contribute
                    </button>
                  </div>
                  <ContributeDialog
                    open={contributing()}
                    items={entries().map((entry) => ({
                      key: entry.name,
                      name: entry.name,
                      bytes: entry.bytes,
                    }))}
                    upload={contribute}
                    onClose={() => setContributing(false)}
                  >
                    <p class="cd-public">Your message will be posted publicly on GitHub.</p>
                    <Show when={fromUsb() && connected() && connection.remove}>
                      <label class="cd-check">
                        <input
                          type="checkbox"
                          checked={remove()}
                          onChange={(event) => setRemove(event.currentTarget.checked)}
                        />
                        Delete from camera after uploading
                      </label>
                    </Show>
                  </ContributeDialog>
                  <Show when={connected()}>
                    <button
                      class="pill"
                      disabled={busy()}
                      onClick={() => {
                        handedOff = true;
                        connection.handOff().then(
                          () => setDevTools(true),
                          (cause: unknown) => problem(cause),
                        );
                      }}
                    >
                      Open Dev Tools
                    </button>
                  </Show>
                </section>
              </>
            )}
          </Show>
          <p role="status" aria-live="polite">
            {status()}
          </p>
          <Show when={error()}>
            <p role="alert">{error()}</p>
          </Show>
        </main>
      }
    >
      <DevTools model={untrack(() => props.devToolsModel)} supported={supported} />
    </Show>
  );
}
