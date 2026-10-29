import { For, Show, createEffect, createSignal } from 'solid-js';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import { formatBytes } from '@fujimod/emulator-ui/ui/file-list';
import type { Camera, CameraModel } from '../camera-model';
import { executionSelection } from '../captures/capture';
import type { AddFile } from '../../files/shelf';
import { ContributeDialog } from '../../files/ContributeDialog';
import type { DeviceFileEntry } from '../../device-files';
import type { ReportLink } from '../../bug/report-link';
import { findReportService, listReports, removeReport, retrieveListed } from '../../bug/transfer';
import { uploadReport, type UploadReport } from '../../bug/upload';
import './reports.css';

/** The firmware whose report folder and file service Dev Tools knows. */
const REPORT_FIRMWARE = 'x70-110';

type Scan =
  | { kind: 'scanning' }
  | { kind: 'unsupported' }
  | { kind: 'found'; service: ReportLink; entries: DeviceFileEntry[] }
  | { kind: 'failed'; error: string };

interface ReadReport {
  name: string;
  blob: Blob;
  link: ReportLink;
  /** Stable across upload retries, so the server never files the same report twice. */
  uploadId: string;
  uploaded?: boolean;
}

/** Finds the issue reports on the camera's card when it connects, and files each as its own issue. */
export function BugReports(props: {
  camera: Camera;
  model: CameraModel;
  onFile: AddFile;
  upload?: UploadReport;
}) {
  const [scans, setScans] = createSignal(new Map<string, Scan>());
  const [running, setRunning] = createSignal('');
  const [progress, setProgress] = createSignal('');
  const [status, setStatus] = createSignal('');
  const [reports, setReports] = createSignal<ReadReport[]>([]);
  const [contributing, setContributing] = createSignal(false);
  const [remove, setRemove] = createSignal(true);
  const [wiggle, setWiggle] = createSignal(false);
  const announced = new Set<string>();
  const scan = () => scans().get(props.camera.id);
  const firmware = () => executionSelection(props.camera.identity)?.firmware.id;
  const available = () =>
    props.camera.present && !props.camera.uncertain && !props.camera.session && !running();
  function setScan(id: string, value: Scan | undefined) {
    setScans((map) => {
      const next = new Map(map);
      if (value) next.set(id, value);
      else next.delete(id);
      return next;
    });
  }

  async function find(id: string) {
    setScan(id, { kind: 'scanning' });
    try {
      const found = await props.model.withSession(id, 'Find bug reports', async (session) => {
        const memory = await session.memory(REPORT_FIRMWARE);
        try {
          const service = await findReportService(memory);
          return service && { service, entries: await listReports(memory, service) };
        } finally {
          await memory.close();
        }
      });
      setScan(id, found ? { kind: 'found', ...found } : { kind: 'unsupported' });
      if (found?.entries.length && !announced.has(id)) {
        announced.add(id);
        setWiggle(true);
      }
    } catch (error) {
      setScan(id, { kind: 'failed', error: shownError(error) });
    }
  }
  // Look once per connection; a camera that leaves is looked at again when it returns.
  createEffect(
    () => ({
      id: props.camera.id,
      present: props.camera.present,
      firmware: firmware(),
      ready: available(),
    }),
    ({ id, present, firmware, ready }) => {
      if (!present) {
        setScan(id, undefined);
        announced.delete(id);
      } else if (firmware === REPORT_FIRMWARE && ready && !scans().has(id)) void find(id);
    },
  );

  async function reportAll() {
    const current = scan();
    if (current?.kind !== 'found' || !available()) return;
    const { service, entries } = current;
    const id = props.camera.id;
    setRunning('Read bug reports');
    setStatus('');
    try {
      const total = entries.reduce((sum, entry) => sum + entry.bytes, 0);
      let before = 0;
      const read = await props.model.withSession(id, 'Read bug reports', async (session) => {
        const memory = await session.memory(REPORT_FIRMWARE);
        try {
          const list: ReadReport[] = [];
          for (const entry of entries) {
            const report = await retrieveListed(memory, service, entry, (bytes) =>
              setProgress(`Reading ${Math.floor(((before + bytes) / total) * 100)}%`),
            );
            before += entry.bytes;
            list.push({ name: entry.name, ...report, uploadId: crypto.randomUUID() });
          }
          return list;
        } finally {
          await memory.close();
        }
      });
      for (const report of read)
        props.onFile(`bug-report-${report.name.toLowerCase()}`, report.blob, {
          path: report.link.path,
          bytes: report.link.bytes,
        });
      setReports(read);
      setContributing(true);
    } catch (error) {
      setStatus(shownError(error));
    } finally {
      setProgress('');
      setRunning('');
    }
  }

  /** One issue per report; deletes only reports the server confirmed it stored. */
  async function contribute(message: string, fraction: (value: number) => void) {
    const upload = props.upload ?? uploadReport;
    const list = reports();
    let last: { id: string; shareUrl: string; issueURL?: string } | undefined;
    for (const [index, report] of list.entries()) {
      fraction(index / list.length);
      if (report.uploaded) continue;
      last = await upload(report.blob, report.link, message, report.uploadId);
      report.uploaded = true;
    }
    fraction(1);
    const id = props.camera.id;
    if (remove() && props.camera.present) {
      await props.model.withSession(id, 'Delete bug reports', async (session) => {
        const memory = await session.memory(REPORT_FIRMWARE);
        try {
          for (const report of list) if (report.uploaded) await removeReport(memory, report.link);
        } finally {
          await memory.close();
        }
      });
      setScan(id, undefined);
    }
    return {
      id: last?.id ?? list[0].uploadId,
      shareUrl: last?.shareUrl ?? '',
      link: {
        href: 'https://github.com/justjake/fujimod/issues?q=label%3Afrom-camera',
        label: list.length === 1 ? 'View GitHub issue' : 'View GitHub issues',
      },
    };
  }

  return (
    <Show when={firmware() === REPORT_FIRMWARE && scan()?.kind !== 'unsupported'}>
      <section
        class={{ 'wb-section': true, 'br-wiggle': wiggle() }}
        aria-labelledby="br-label"
        onAnimationEnd={() => setWiggle(false)}
      >
        <h3 id="br-label" class="wb-label">
          Bug reports
        </h3>
        <div class="wb-tool">
          <Show
            when={scan()?.kind === 'found' && scan()}
            fallback={
              <p class="br-empty">
                {scan()?.kind === 'failed'
                  ? (scan() as { error: string }).error
                  : 'Looking for bug reports…'}
              </p>
            }
          >
            {(found) => {
              const entries = () => (found() as Extract<Scan, { kind: 'found' }>).entries;
              return (
                <Show
                  when={entries().length}
                  fallback={<p class="br-empty">No bug reports on the card.</p>}
                >
                  <ul class="br-list">
                    <For each={entries()}>
                      {(entry) => (
                        <li>
                          <span>{entry.name}</span>
                          <span class="br-size">{formatBytes(entry.bytes)}</span>
                        </li>
                      )}
                    </For>
                  </ul>
                  <div class="br-actions">
                    <button
                      class="wb-btn wb-primary"
                      disabled={!available()}
                      onClick={() => void reportAll()}
                    >
                      {running()
                        ? progress() || 'Reading…'
                        : entries().length === 1
                          ? 'Report bug'
                          : `Report ${entries().length} bugs`}
                    </button>
                  </div>
                </Show>
              );
            }}
          </Show>
        </div>
        <Show when={status()}>
          <p class="wb-status" role="alert">
            {status()}
          </p>
        </Show>
        <ContributeDialog
          open={contributing()}
          items={reports().map((report) => ({
            key: report.name,
            name: report.name,
            bytes: report.blob.size,
          }))}
          upload={contribute}
          onClose={() => setContributing(false)}
        >
          <p class="cd-public">Your message will be posted publicly on GitHub.</p>
          <label class="cd-check">
            <input
              type="checkbox"
              checked={remove()}
              onChange={(event) => setRemove(event.currentTarget.checked)}
            />
            Delete from camera after uploading
          </label>
        </ContributeDialog>
      </section>
    </Show>
  );
}
