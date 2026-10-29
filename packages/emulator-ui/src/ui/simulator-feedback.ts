import type { EngineFault } from '../engine/fault';
import { faultText, type FaultContext } from './fault-text';

export interface SimulatorIssueContext {
  event: 'feedback' | 'crash';
  camera?: { model: string; id: string };
  fault?: EngineFault;
  faultContext?: FaultContext;
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
    url.searchParams.set(
      'crash',
      faultText(
        context.fault,
        context.faultContext ?? {
          firmware: camera?.id,
          build: context.simulatorBuild,
        },
      ),
    );
  }
  return { description, issue: { url: url.href, fields: Object.fromEntries(url.searchParams) } };
}
