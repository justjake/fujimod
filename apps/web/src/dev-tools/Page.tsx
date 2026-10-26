import { getRequestEvent } from '@solidjs/web';
import { createSignal, onSettled, untrack } from 'solid-js';
import type { ReportSubmission } from '@fujimod/emulator-ui/engine/report';
import { errorReportZip } from '@fujimod/emulator-ui/engine/report';
import { LocalReports } from '@fujimod/emulator-ui/ui/local-reports';
import type { CameraModel } from './camera-model';
import { toolAt, type Wizard } from './routes';
import { remoteControlUrl } from './sharing';
import { pageCameraModel } from './usb-camera-model';
import { Workbench } from './Workbench';
import '../home/site.css';
import './style.css';

declare const __FUJIMOD_SIMULATOR_BUILD__: string;

/** A `/dev-tools/<tool>/` path opens one of the guided workflows directly. */
function initialWizard(): Wizard | undefined {
  const url =
    getRequestEvent()?.request.url ?? (typeof window === 'undefined' ? undefined : location.href);
  return url ? toolAt(new URL(url).pathname) : undefined;
}

/** The crash and feedback report: the log, with each entry's bytes as a binary file. */
async function report(model: CameraModel, submission: Promise<ReportSubmission>) {
  const details = await submission;
  const entries = model.log();
  const attachments: [string, Uint8Array<ArrayBuffer>][] = entries
    .filter((entry) => entry.data?.length)
    .map((entry) => [`usb/${entry.id}.bin`, new Uint8Array(entry.data!)]);
  return errorReportZip(
    {
      schema: 'fujimod-error-report-v2',
      createdAt: new Date().toISOString(),
      source: 'dev-tools',
      cameras: model.cameras().map((camera) => ({
        name: camera.name,
        transport: camera.transport,
        present: camera.present,
        uncertain: camera.uncertain,
        sharing: Boolean(camera.sharing),
        identity: camera.identity,
        problem: camera.problem,
      })),
      logs: entries.map((entry) => ({
        ...entry,
        time: new Date(entry.time).toISOString(),
        data: undefined,
        attachment: entry.data?.length ? `usb/${entry.id}.bin` : undefined,
      })),
      browser: { userAgent: navigator.userAgent },
      ...details,
    },
    undefined,
    attachments,
  );
}

export default function DevTools(props: { model?: CameraModel; supported?: boolean }) {
  const model = untrack(() => props.model) ?? pageCameraModel();
  // Server renders have no navigator, so decide once mounted; hydration keeps the
  // server's initial value otherwise.
  const [usbSupported, setUsbSupported] = createSignal(untrack(() => props.supported) ?? false);
  const reports = new LocalReports(
    'dev-tools',
    () => ({
      event: model.cameras().some((camera) => camera.uncertain || camera.problem)
        ? 'crash'
        : 'feedback',
      diagnostics: { cameras: model.cameras().length },
      modBuild: 'Stock firmware (no mods)',
      simulatorBuild: __FUJIMOD_SIMULATOR_BUILD__,
    }),
    (submission) => report(model, submission),
  );
  onSettled(() => {
    if (untrack(() => props.supported) === undefined)
      setUsbSupported(isSecureContext && 'usb' in navigator);
    const unlisten = reports.listen();
    // A share link opens the shared camera as a remote camera.
    if (
      remoteControlUrl(location.href) &&
      !model.cameras().some((camera) => camera.transport === 'remote')
    )
      void model.connectRemote(location.href).catch((error: unknown) => reports.crash(error));
    return unlisten;
  });
  return (
    <div class="shell dev-tools">
      <header>
        <a href="../" aria-label="Fujimod">
          <img src="../brand.png" alt="Fujimod" width="110" height="28" />
        </a>
        <nav aria-label="Pages">
          <a href="../">Emulator</a>
          <a href="./" aria-current="page">
            Dev Tools
          </a>
          <a href="../recovery/">Recovery</a>
        </nav>
      </header>
      <Workbench model={model} usbSupported={usbSupported()} initialWizard={initialWizard()} />
    </div>
  );
}
