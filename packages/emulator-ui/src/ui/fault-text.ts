import type { EngineFault } from '../engine/fault';

export interface FaultContext {
  tick?: number;
  firmware?: string;
  build: string;
}
/** A fault detail as display text; objects are shown as JSON rather than [object Object]. */
function detail(value: unknown): string {
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean' || typeof value === 'bigint')
    return String(value);
  if (value == null) return '';
  return JSON.stringify(value) ?? '';
}
export function faultDiagnostics(fault: EngineFault, context: FaultContext) {
  const d = fault.details;
  const kind = detail(d.type ?? d.error_type ?? fault.kind);
  const access = [
    d.operation,
    d.address,
    d.value == null ? '' : `← ${detail(d.value)}`,
    d.device ? `(${detail(d.device)})` : '',
  ]
    .filter(Boolean)
    .join(' ');
  const raw = detail(d.error ?? d.message);
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
    `pc ${detail(d.pc ?? 'unknown')} · core ${detail(d.cpu ?? 'unknown')} · tick ${detail(context.tick ?? d.tick ?? 'unknown')} · firmware ${context.firmware ?? 'unknown'} · build ${context.build} · ${browser} / ${os}`,
  ];
  return lines.join('\n');
}
export const faultHeading = 'The emulator stopped';
export function faultMessage(fault: EngineFault) {
  return fault.kind === 'unsupported-device'
    ? "We don't know how to simulate this camera hardware feature yet. It works fine on a real camera."
    : fault.message;
}
export function faultText(fault: EngineFault, context: FaultContext) {
  return [faultHeading, faultMessage(fault), faultDiagnostics(fault, context)].join('\n');
}
