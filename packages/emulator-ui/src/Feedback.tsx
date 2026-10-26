import type { JSX } from '@solidjs/web';
import { createEffect, createSignal, onSettled } from 'solid-js';
import type { SessionController } from './session/controller';
import type { ReportSubmission } from './engine/report';
import { simulatorIssue } from './ui/simulator-feedback';
import {
  readPendingReport,
  storePendingReport,
  deliverReport,
  type PendingReport,
} from './ui/report-delivery';
import { downloadBlob } from './ui/download';

export interface EmulatorFeedbackActions {
  readonly ready: boolean;
  readonly saved: boolean;
  open(): void;
  recover(): void;
}
export function EmulatorFeedback(props: {
  runtime?: SessionController;
  request?: { event: 'feedback' | 'crash' };
  children: (actions: EmulatorFeedbackActions) => JSX.Element;
}) {
  let dialog!: HTMLDialogElement;
  let describe: ((value: ReportSubmission) => void) | undefined;
  let capture: Promise<{ blob: Blob } | { error: unknown }>;
  let context: ReturnType<SessionController['context']>;
  let submitted: ReturnType<typeof simulatorIssue> | undefined, pending: PendingReport | undefined;
  let closed = true;
  const [description, setDescription] = createSignal(''),
    [preview, setPreview] = createSignal('');
  const [busy, setBusy] = createSignal(false),
    [locked, setLocked] = createSignal(false);
  const [status, setStatus] = createSignal(''),
    [label, setLabel] = createSignal('Download and report');
  const [issue, setIssue] = createSignal(''),
    [saved, setSaved] = createSignal<PendingReport>();
  const [local, setLocal] = createSignal(false);
  onSettled(() => {
    void readPendingReport().then((report) => setSaved(report));
    return () => {
      closed = true;
      describe?.(simulatorIssue(description(), context));
    };
  });
  function open(event: 'feedback' | 'crash') {
    const runtime = props.runtime;
    if (!runtime || busy() || dialog.open) return;
    context = runtime.context(event);
    setDescription('');
    setStatus('');
    setIssue('');
    setLocked(false);
    setPreview(runtime.viewer.captureView());
    dialog.returnValue = 'cancel';
    const submission = new Promise<ReportSubmission>((resolve) => {
      describe = resolve;
    });
    capture = runtime.capture(submission).then(
      (blob) => ({ blob }),
      (error) => ({ error }),
    );
    submitted = pending = undefined;
    closed = false;
    const available = runtime.localReports.available();
    setLocal(available);
    setLabel(available ? 'Save report' : 'Download and report');
    dialog.showModal();
    dialog.querySelector('textarea')?.focus();
  }
  createEffect(
    () => props.request,
    (request) => {
      if (request) open(request.event);
    },
  );
  async function submit(event: SubmitEvent) {
    if ((event.submitter as HTMLButtonElement | null)?.value !== 'download') return;
    event.preventDefault();
    if (busy()) return;
    setBusy(true);
    setLocked(true);
    submitted ??= simulatorIssue(description(), context);
    describe?.(submitted);
    describe = undefined;
    setStatus('Preparing report…');
    try {
      if (local()) {
        await props.runtime!.localReports.submit(
          submitted,
          async () => {
            const report = await capture;
            if ('error' in report) throw report.error;
            return report.blob;
          },
          context,
        );
        if (!closed) {
          setStatus('Saved locally');
          setLabel('Save again');
        }
        return;
      }
      if (!pending) {
        const report = await capture;
        if (closed) return;
        if ('error' in report) throw report.error;
        pending = {
          blob: report.blob,
          name: `fujimod-feedback-${Date.now()}.zip`,
          issueURL: submitted.issue.url,
        };
      }
      await storePendingReport(pending);
      if (closed) return;
      setSaved(pending);
      setIssue(pending.issueURL);
      setStatus('');
      deliverReport(pending, downloadBlob);
      setLabel('Download again');
    } catch (error) {
      if (!closed) setStatus(`Could not prepare report: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  }
  const actions: EmulatorFeedbackActions = {
    get ready() {
      return !!props.runtime;
    },
    get saved() {
      return !!saved();
    },
    open: () => open('feedback'),
    recover: () => {
      const report = saved();
      if (report) downloadBlob(report.blob, report.name);
    },
  };
  return (
    <>
      {props.children(actions)}
      <dialog
        ref={dialog}
        id="feedback-dialog"
        aria-labelledby="feedback-title"
        onClose={() => {
          closed = true;
          describe?.(submitted ?? simulatorIssue(description(), context));
          describe = undefined;
        }}
      >
        <form method="dialog" onSubmit={submit}>
          <h3 id="feedback-title">Feedback</h3>
          <div class="feedback-content">
            <figure>
              <img
                id="feedback-preview"
                src={preview() || undefined}
                alt="Camera view when this report was opened"
              />
              <figcaption>Report includes simulator state</figcaption>
            </figure>
            <div>
              <textarea
                aria-label="Feedback (optional)"
                id="feedback-description"
                rows="5"
                placeholder="Share a bug, idea, or other feedback…"
                value={description()}
                onInput={(event) => setDescription(event.currentTarget.value)}
                readonly={locked()}
              />
            </div>
          </div>
          <p id="feedback-status" role="status" hidden={!status()}>
            {status()}
          </p>
          <div class="act">
            <button class="pill" value="cancel">
              Cancel
            </button>
            <button class="pill primary" id="feedback-prepare" value="download" disabled={busy()}>
              {label()}
            </button>
            <a
              class="pill"
              id="feedback-issue"
              target="_blank"
              rel="noopener noreferrer"
              href={issue() || undefined}
              hidden={!issue()}
            >
              Open GitHub
            </a>
          </div>
        </form>
      </dialog>
    </>
  );
}
