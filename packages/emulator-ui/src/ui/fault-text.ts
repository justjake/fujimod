import type { EngineFault } from '../engine/fault';

export interface FaultContext {
  tick?: number;
  firmware?: string;
  build: string;
}
export function faultDiagnostics(fault: EngineFault, context: FaultContext) {
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
export const faultHeading = 'The emulator stopped';
export function faultMessage(fault: EngineFault) {
  return fault.kind === 'unsupported-device'
    ? "We don't know how to simulate this camera hardware feature yet. It works fine on a real camera."
    : fault.message;
}
export function faultText(fault: EngineFault, context: FaultContext) {
  return [faultHeading, faultMessage(fault), faultDiagnostics(fault, context)].join('\n');
}
