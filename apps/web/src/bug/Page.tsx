import { Show, For, createSignal, onSettled, untrack } from 'solid-js';
import DevTools from '../dev-tools/Page';
import type { CameraModel } from '../dev-tools/camera-model';
import { downloadBlob } from '@fujimod/emulator-ui/ui/download';
import { FileList } from '@fujimod/emulator-ui/ui/file-list';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import { FileDrop } from '../ui/file-drop';
import { SiteHeader } from '../ui/site-header';
import { ContributeDialog } from '../files/ContributeDialog';
import { uploadReport, type UploadReport, type UploadReceipt } from './upload';
import { reportPlatform, type ReportPlatform } from './file-guide';
import {
  parseReportLink,
  decode64,
  validateReport,
  reportName,
  reportEntries,
  reportDetails,
  capturedText,
  type ReportDetails,
  type ReportEntry,
  type ReportLink,
} from './report-link';
import type { DeviceFileEntry } from '../device-files';
import { formatBytes } from '@fujimod/emulator-ui/ui/file-list';
import { prepareDownload } from './download';
import { ReportUsb, type CameraChoice, type ReportConnection } from './usb';
import '../home/site.css';
import '../navigation.css';
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
  /** The loaded report: the QR code's, or one listed on the camera. */
  const [current, setCurrent] = createSignal<ReportLink>();
  const [details, setDetails] = createSignal<ReportDetails>({});
  const [screen, setScreen] = createSignal<string>();
  const [others, setOthers] = createSignal<DeviceFileEntry[]>([]);
  const [contributed, setContributed] = createSignal<ReadonlySet<string>>(new Set());
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
      const url = untrack(screen);
      if (url) URL.revokeObjectURL(url);
      if (!handedOff) connection.dispose();
    };
  });
  function problem(cause: unknown) {
    setError(shownError(cause));
    setBusy(false);
  }
  async function acceptArchive(blob: Blob, report: ReportLink) {
    const bytes = new Uint8Array(await blob.arrayBuffer());
    const found = reportEntries(bytes, report);
    const shown = reportDetails(bytes, found);
    const previous = screen();
    if (previous) URL.revokeObjectURL(previous);
    setScreen(
      shown.screen
        ? URL.createObjectURL(
            new Blob(
              [bytes.subarray(shown.screen.offset, shown.screen.offset + shown.screen.bytes)],
              {
                type: 'image/bmp',
              },
            ),
          )
        : undefined,
    );
    setDetails(shown);
    setEntries(found);
    setCurrent(report);
    setArchive(blob);
    setReceipt(undefined);
    uploadId = crypto.randomUUID();
    setDownload(undefined);
    setStatus('Preparing download…');
    setDownload(await prepareDownload(blob, reportName(report)));
  }
  async function refreshList() {
    if (!connection.list) return;
    try {
      setOthers(await connection.list());
    } catch (cause) {
      setOthers([]);
      setError(`Could not list other reports: ${shownError(cause)}`);
    }
  }
  async function openListed(entry: DeviceFileEntry) {
    if (!connection.readListed || busy()) return;
    setBusy(true);
    setError('');
    setStatus('Reading report…');
    try {
      const report = await connection.readListed(entry, (bytes) =>
        setStatus(`Reading report… ${Math.floor((bytes / entry.bytes) * 100)}%`),
      );
      await acceptArchive(report.blob, report.link);
      setFromUsb(true);
      setStatus('Report retrieved from camera.');
    } catch (cause) {
      problem(cause);
    } finally {
      setBusy(false);
    }
  }
  const loadedName = () => (current() ? reportName(current()!) : undefined);
  /** The next listed report not yet contributed from this page. */
  const nextReport = () =>
    others().find((entry) => entry.name !== loadedName() && !contributed().has(entry.name));
  async function read(id: number) {
    setBusy(true);
    setError('');
    setStatus('Reading report…');
    const report = link()!;
    try {
      await acceptArchive(
        await connection.read(id, report, (bytes) =>
          setStatus(`Reading report… ${Math.floor((bytes / report.bytes) * 100)}%`),
        ),
        report,
      );
      setConnected(true);
      setFromUsb(true);
      setStatus('Report retrieved from camera.');
      await refreshList();
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
  async function chooseFile(file: File | undefined) {
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
    const report = current();
    if (!archive() || !report) throw Error('No report is loaded.');
    setBusy(true);
    setError('');
    progress(0);
    try {
      const result = await upload(archive()!, report, message, uploadId);
      progress(1);
      setReceipt(result);
      setContributed((names) => new Set([...names, reportName(report)]));
      const confirmation = result.issueURL
        ? 'Report uploaded and GitHub issue created.'
        : 'Report uploaded. GitHub issue creation is pending.';
      setStatus(confirmation);
      if (fromUsb() && connected() && remove() && connection.remove) {
        try {
          await connection.remove(report);
          setStatus(confirmation + ' Deleted from camera.');
          await refreshList();
        } catch (cause) {
          shownError(cause);
          setError('Your report was uploaded, but could not be deleted from the camera.');
        }
      }
      return {
        id: result.id,
        shareUrl: result.shareUrl,
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
        <div class="shell bug-report">
          <SiteHeader root="../" />
          <main>
            <Show when={link()}>
              {(value) => (
                <>
                  <p>
                    This web app uses WebUSB to read issue reports from a FujiMod camera showing the
                    issue QR code. It also works if USB debugging is enabled.
                  </p>
                  <p>
                    <strong>
                      When your camera is showing the QR code, it’s ready to upload your error
                      report over USB.
                    </strong>
                  </p>
                  <div class="bug-paths">
                    <section
                      class={{
                        'bug-card': true,
                        'bug-usb-unavailable': !supported,
                      }}
                      aria-label="USB connection"
                    >
                      <Show when={!supported}>
                        <h2 class="bug-unavailable-title">WebUSB unavailable</h2>
                        <p class="bug-unavailable-reason">
                          Use Chrome or Edge on a computer, or choose the TAR file from your SD
                          card.
                        </p>
                      </Show>
                      <Show when={supported}>
                        <h2>Connect over USB</h2>
                        <p>
                          Leave the QR screen open and plug in USB. If it was already plugged in,
                          unplug and reconnect it.
                        </p>
                        <p class="bug-wait">
                          <Show when={busy()}>
                            <span class="bug-spinner" aria-hidden="true" />
                          </Show>
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
                            Open the issue QR screen again, or enable SET UP → FUJIMOD DEVTOOLS →
                            USB DEBUGGING, then reconnect USB.
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
                      <FileDrop
                        inputId="bug-report-file"
                        title={reportName(value())}
                        prompt="Drop it here, or"
                        accept=".tar,application/x-tar"
                        label="Choose report TAR"
                        highlighted={!supported}
                        disabled={busy()}
                        onFile={(file) => void chooseFile(file)}
                      />
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
                      <Show when={details().capturedAt}>
                        {(time) => <p class="bug-captured">Captured {capturedText(time())}</p>}
                      </Show>
                      <Show when={screen()}>
                        {(url) => (
                          <img
                            class="bug-screen"
                            src={url()}
                            width="720"
                            height="480"
                            alt="Camera screen when the report was captured"
                          />
                        )}
                      </Show>
                      <p class="bug-file">
                        {loadedName()} · {archive()!.size.toLocaleString()} bytes
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
                      next={
                        nextReport() && connected()
                          ? {
                              label: 'Next report',
                              run: () => {
                                const entry = nextReport()!;
                                setContributing(false);
                                void openListed(entry).then(() => {
                                  if (current() && reportName(current()!) === entry.name)
                                    setContributing(true);
                                });
                              },
                            }
                          : undefined
                      }
                    >
                      <p class="cd-public">Your message will be posted publicly on GitHub.</p>
                      <Show when={fromUsb() && connected() && connection.remove !== undefined}>
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
                  <Show when={connected() && others().length > 1}>
                    <section class="bug-card bug-others" aria-label="Reports on this camera">
                      <h2>Reports on this camera</h2>
                      <ul>
                        <For each={others()}>
                          {(entry) => (
                            <li>
                              <span class="bug-file">{entry.name}</span>
                              <span class="bug-dim">{formatBytes(entry.bytes)}</span>
                              <Show
                                when={entry.name !== loadedName()}
                                fallback={<span class="bug-dim">Showing</span>}
                              >
                                <Show
                                  when={!contributed().has(entry.name)}
                                  fallback={
                                    <span class="bug-dim">
                                      <Icon name="check" size={13} /> Contributed
                                    </span>
                                  }
                                >
                                  <button
                                    class="pill"
                                    disabled={busy()}
                                    onClick={() => void openListed(entry)}
                                  >
                                    Open
                                  </button>
                                </Show>
                              </Show>
                            </li>
                          )}
                        </For>
                      </ul>
                    </section>
                  </Show>
                </>
              )}
            </Show>
            <p class="bug-status" role="status" aria-live="polite">
              {status()}
            </p>
            <Show when={error()}>
              <p role="alert">{error()}</p>
            </Show>
          </main>
        </div>
      }
    >
      <DevTools model={untrack(() => props.devToolsModel)} supported={supported} />
    </Show>
  );
}
