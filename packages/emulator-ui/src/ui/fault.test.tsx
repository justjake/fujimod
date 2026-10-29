import { render } from '@solidjs/web';
import { expect, test } from 'vite-plus/test';
import { FaultPanel } from './fault';
import { simulatorIssue } from './simulator-feedback';
import type { EngineFault } from '../engine/fault';

test.each(['unsupported-device', 'guest-fault', 'wasm-trap', 'exception'] as const)(
  'GitHub includes every notification line for %s without shortening it',
  (kind) => {
    const fault: EngineFault = {
      kind,
      message: 'Execution fault 🦊 & details',
      details: {
        error_type: 'DeviceFailure',
        error: `First line\n${'diagnostic '.repeat(100)}`,
        pc: '0x1234',
        cpu: 'ARM',
      },
    };
    const faultContext = { tick: 123456, firmware: 'x70-110', build: 'v42' };
    const container = document.createElement('div');
    const dispose = render(
      () => (
        <FaultPanel
          fault={fault}
          context={faultContext}
          canUndo={false}
          undoBusy={false}
          canRestart={false}
          undo={() => {}}
          restart={() => {}}
          report={() => {}}
        />
      ),
      container,
    );
    try {
      const notification = ['#fault-heading', '#fault-message', '#fault-raw']
        .map((selector) => container.querySelector(selector)!.textContent!.trim())
        .join('\n');
      const submission = simulatorIssue('My description', {
        event: 'crash',
        fault,
        faultContext,
        simulatorBuild: 'simulator-version',
        modBuild: 'mod-version',
      });
      const query = new URL(submission.issue.url).searchParams;
      expect(query.get('crash')!.trim()).toBe(notification);
      expect(query.get('crash')).toContain('diagnostic '.repeat(100).trim());
      expect(query.get('crash')).toContain(navigator.platform);
      expect(query.get('feedback')).toBe('My description');
    } finally {
      dispose();
    }
  },
);
