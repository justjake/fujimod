import type { EngineFault } from '../engine/fault';

export interface SimulatorIssueContext {
  event: 'feedback' | 'crash';
  camera?: { model: string; id: string };
  fault?: EngineFault;
  modBuild: string;
  simulatorBuild: string;
}

/** Both entry points use the same template, with context captured at report time. */
export function simulatorIssue(description: string, context: SimulatorIssueContext) {
  const url = new URL('https://github.com/justjake/fujimod/issues/new');
  const labels = ['Simulator'];
  if (context.event === 'crash') labels.push('bug');
  if (context.camera) labels.push(`Camera: ${context.camera.model}`);
  url.searchParams.set('template', `simulator-${context.event}.yml`);
  url.searchParams.set('labels', labels.join(','));
  const camera = context.camera;
  url.searchParams.set(
    'title',
    `[Simulator${camera ? ` · ${camera.model}` : ''}] ${context.event === 'crash' ? 'Crash' : 'Feedback'}`,
  );
  url.searchParams.set('feedback', description.trim());
  url.searchParams.set('firmware', camera?.id ?? 'Not identified');
  url.searchParams.set('mod-build', context.modBuild);
  url.searchParams.set('simulator-build', context.simulatorBuild);
  if (context.event === 'crash' && context.fault) {
    const fault = context.fault;
    // Keep the full diagnostic data in the ZIP; include only a brief summary in the URL.
    const details = ['type', 'device', 'operation', 'address', 'pc', 'cpu']
      .filter((key) => fault.details[key] != null)
      .map((key) => `${key}: ${String(fault.details[key]).slice(0, 160)}`);
    url.searchParams.set('crash', `${fault.kind}: ${fault.message}\n${details.join('\n')}`);
  }
  return { description, issue: { url: url.href, fields: Object.fromEntries(url.searchParams) } };
}
