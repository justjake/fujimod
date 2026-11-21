import { render } from '@solidjs/web';
import { invoke } from '@tauri-apps/api/core';
import App from './App';
import type { DesktopBridge } from './launch';

if ('__TAURI_INTERNALS__' in window) {
  (window as Window & { fujimodDesktop?: DesktopBridge }).fujimodDesktop = {
    configurations: () => invoke('configurations'),
    chooseUpdate: () => invoke('choose_update'),
    launch: (request) => invoke('launch', { request }),
    close: () => invoke('close'),
  };
}

render(() => <App />, document.getElementById('root')!);
