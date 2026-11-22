import { render } from '@solidjs/web';
import { invoke } from '@tauri-apps/api/core';
import App from './App';
import type { DesktopBridge } from './launch';

function showFatal(error: unknown) {
  const text = error instanceof Error ? (error.stack ?? error.message) : String(error);
  const report = document.createElement('pre');
  report.className = 'page-error';
  report.textContent = text;
  document.body.append(report);
}

addEventListener('error', (event) => showFatal(event.error ?? event.message));
addEventListener('unhandledrejection', (event) => showFatal(event.reason));

if ('__TAURI_INTERNALS__' in window) {
  (window as Window & { fujimodDesktop?: DesktopBridge }).fujimodDesktop = {
    configurations: () => invoke('configurations'),
    chooseUpdate: () => invoke('choose_update'),
    launch: (request) => invoke('launch', { request }),
    launchConnection: () => invoke('launch_connection'),
    close: () => invoke('close'),
  };
}

try {
  render(() => <App />, document.getElementById('root')!);
} catch (error) {
  showFatal(error);
}
