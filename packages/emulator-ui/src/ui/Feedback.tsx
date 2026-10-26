import { createSignal, onSettled } from 'solid-js';
import { simulatorIssue, type SimulatorIssueContext } from './simulator-feedback';
import { LocalReports, type ReportCapture } from './local-reports';
import { downloadBlob } from './download';
import { deliverReport, storePendingReport } from './report-delivery';
import type { ReportSubmission } from '../engine/report';
import './feedback.css';
export function Feedback(props: {
  context: () => SimulatorIssueContext;
  capture: ReportCapture;
  local: LocalReports;
}) {
  let dialog!: HTMLDialogElement;
  let description!: HTMLTextAreaElement;
  let context: SimulatorIssueContext;
  let describe: ((value: ReportSubmission) => void) | undefined;
  let capture: Promise<{ blob: Blob } | { error: unknown }>;
  const [busy, setBusy] = createSignal(false),
    [status, setStatus] = createSignal('');
  const [local, setLocal] = createSignal(false);
  onSettled(() => {
    setLocal(props.local.available());
  });
  function open() {
    context = props.context();
    description.value = '';
    setStatus('');
    const submission = new Promise<ReportSubmission>((resolve) => {
      describe = resolve;
    });
    capture = props.capture(submission).then(
      (blob) => ({ blob }),
      (error) => ({ error }),
    );
    dialog.showModal();
    description.focus();
  }
  async function submit(event: SubmitEvent) {
    if ((event.submitter as HTMLButtonElement)?.value !== 'report') return;
    event.preventDefault();
    if (busy()) return;
    setBusy(true);
    const submission = simulatorIssue(description.value, context);
    describe?.(submission);
    describe = undefined;
    setStatus('Preparing report…');
    try {
      if (props.local.available()) {
        await props.local.submit(
          submission,
          async () => {
            const result = await capture;
            if ('error' in result) throw result.error;
            return result.blob;
          },
          context,
        );
        setStatus('Saved locally');
      } else {
        const result = await capture;
        if ('error' in result) throw result.error;
        const pending = {
          blob: result.blob,
          name: `fujimod-feedback-${Date.now()}.zip`,
          issueURL: submission.issue.url,
        };
        await storePendingReport(pending);
        deliverReport(pending, downloadBlob);
        setStatus('Report downloaded');
      }
    } catch (error) {
      setStatus(`Could not save report: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <button class="pill" onClick={open}>
        Feedback
      </button>
      <dialog
        ref={(node) => {
          dialog = node;
        }}
        class="simulator-feedback"
        aria-label="Simulator report"
        onClose={() => {
          describe?.(simulatorIssue(description.value, context));
          describe = undefined;
        }}
      >
        <form method="dialog" onSubmit={submit}>
          <h3>Simulator report</h3>
          <textarea
            ref={(node) => {
              description = node;
            }}
            aria-label="Feedback (optional)"
            rows="4"
            readonly={busy()}
          />
          <p role="status">{status()}</p>
          <div class="act">
            <button class="pill" value="cancel" disabled={busy()}>
              Close
            </button>
            <button class="pill primary" value="report" disabled={busy()}>
              {local() ? 'Save report' : 'Download and report'}
            </button>
          </div>
        </form>
      </dialog>
    </>
  );
}
