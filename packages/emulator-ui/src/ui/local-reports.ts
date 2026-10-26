import { simulatorIssue, type SimulatorIssueContext } from './simulator-feedback';
import { errorReportZip, type ReportSubmission } from '../engine/report';
import { EngineError, faultFrom, type EngineFault } from '../engine/fault';
export interface LocalReportContext extends SimulatorIssueContext {
  diagnostics?: Record<string, unknown>;
}
export type ReportSource = 'emulator' | 'dev-tools';
export type ReportCapture = (submission: Promise<ReportSubmission>) => Promise<Blob>;
export class LocalReports {
  private seen = new WeakSet<object>();
  constructor(
    private source: ReportSource,
    private context: () => LocalReportContext,
    private capture: ReportCapture,
  ) {}
  available() {
    return (
      typeof document !== 'undefined' &&
      Boolean(document.querySelector('meta[name="fujimod-local-reports"]'))
    );
  }
  async submit(
    submission: ReportSubmission,
    capture: Promise<Blob> | (() => Promise<Blob>),
    context = this.context(),
  ) {
    const endpoint = document.querySelector<HTMLMetaElement>(
      'meta[name="fujimod-local-reports"]',
    )?.content;
    if (!endpoint) return false;
    const id = crypto.randomUUID();
    const result = await fetch(endpoint, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        id,
        source: this.source,
        createdAt: new Date().toISOString(),
        page: location.href,
        browser: { userAgent: navigator.userAgent },
        ...context,
        ...submission,
      }),
    });
    if (!result.ok) throw Error(`Local report metadata failed (${result.status})`);
    let blob: Blob;
    try {
      blob = await (typeof capture === 'function' ? capture() : capture);
    } catch (error) {
      blob = await errorReportZip({
        ...context,
        ...submission,
        snapshot: { unavailable: String(error) },
      });
    }
    const saved = await fetch(`${endpoint}/${id}/report.zip`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/zip' },
      body: blob,
    });
    if (!saved.ok) throw Error(`Local report archive failed (${saved.status})`);
    return true;
  }
  /** Saves a local report when a local sink exists. Otherwise a caught fault is announced
   * as a `fujimod-fault` window event for the host page's error log; uncaught errors
   * (`uncaught`) already reach it through the window's own error events. */
  crash(error: unknown, uncaught = false) {
    if (!this.available()) {
      if (!uncaught)
        dispatchEvent(new CustomEvent<EngineFault>('fujimod-fault', { detail: faultFrom(error) }));
      return;
    }
    const key = error instanceof EngineError ? error.fault : error;
    if (key && typeof key === 'object') {
      if (this.seen.has(key)) return;
      this.seen.add(key);
    }
    const fault: EngineFault = faultFrom(error);
    const context = { ...this.context(), event: 'crash' as const, fault };
    const submission = simulatorIssue('', context);
    // Metadata reaches disk before a potentially expensive snapshot completes.
    void this.submit(submission, () => this.capture(Promise.resolve(submission)), context).catch(
      (error) => console.warn('Could not save simulator report', error),
    );
  }
  listen() {
    const error = (event: ErrorEvent) => this.crash(event.error ?? event.message, true);
    const rejection = (event: PromiseRejectionEvent) => this.crash(event.reason, true);
    document.documentElement.dataset.reportsReady = 'true';
    window.addEventListener('error', error);
    window.addEventListener('unhandledrejection', rejection);
    return () => {
      delete document.documentElement.dataset.reportsReady;
      window.removeEventListener('error', error);
      window.removeEventListener('unhandledrejection', rejection);
    };
  }
}
