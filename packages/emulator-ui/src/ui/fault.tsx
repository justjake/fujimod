import { Show } from 'solid-js';
import type { EngineFault } from '../engine/fault';
import { Icon } from './icons';
export interface FaultContext {
  tick?: number;
  firmware?: string;
  build: string;
}
function diagnostics(fault: EngineFault, context: FaultContext) {
  const d = fault.details;
  const kind = String(d.type ?? d.error_type ?? fault.kind);
  const access = [
    d.operation,
    d.address,
    d.value == null ? '' : `← ${d.value}`,
    d.device ? `(${d.device})` : '',
  ]
    .filter(Boolean)
    .join(' ');
  const raw = String(d.error ?? d.message ?? '');
  const last = raw.trim().split('\n').at(-1) ?? '';
  const ua = typeof navigator === 'undefined' ? '' : navigator.userAgent,
    browser =
      ua.match(/(?:Chrome|Firefox)\/[\d.]+/)?.[0] ??
      `Safari ${ua.match(/Version\/([\d.]+)/)?.[1] ?? ''}`;
  const os =
    ua.match(/iPhone OS ([\d_]+)/)?.[1]?.replaceAll('_', '.') ??
    ua.match(/Mac OS X ([\d_]+)/)?.[1]?.replaceAll('_', '.') ??
    (typeof navigator === 'undefined' ? '' : navigator.platform);
  const lines = [
    `${kind}: ${access || last}`,
    `pc ${d.pc ?? 'unknown'} · core ${d.cpu ?? 'unknown'} · tick ${context.tick ?? d.tick ?? 'unknown'} · firmware ${context.firmware ?? 'unknown'} · build ${context.build} · ${browser} / ${os}`,
  ];
  return lines.join('\n');
}
export function FaultPanel(props: {
  fault?: EngineFault;
  context: FaultContext;
  canUndo: boolean;
  undoBusy: boolean;
  canRestart: boolean;
  undo: () => void;
  restart: () => void;
  report: () => void;
}) {
  return (
    <section class="err" id="engine-status" role="alert" hidden={!props.fault}>
      <h4 id="fault-heading">
        <Icon name="alert" /> The emulator stopped
      </h4>
      <p id="fault-message">
        {props.fault?.kind === 'unsupported-device'
          ? "We don't know how to simulate this camera hardware feature yet. It works fine on a real camera."
          : props.fault?.message}
      </p>
      <code id="fault-raw">{props.fault ? diagnostics(props.fault, props.context) : ''}</code>
      <div class="act">
        <button
          class="pill primary"
          id="undo"
          hidden={!props.canUndo}
          disabled={!props.canUndo || props.undoBusy}
          onClick={() => props.undo()}
        >
          Undo 10 s
        </button>
        <button
          class="pill"
          id="restart"
          disabled={!props.canRestart}
          onClick={() => props.restart()}
        >
          Restart
        </button>
        <button class="pill" id="error-report" onClick={() => props.report()}>
          <Icon name="download" size={14} /> Download and report
        </button>
      </div>
    </section>
  );
}
