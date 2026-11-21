import { invoke } from '@tauri-apps/api/core';
import type { DesktopBridge, LaunchedCamera, LaunchRequest, RunConfiguration } from './launch';

const bridge: DesktopBridge = {
  configurations: () => invoke<RunConfiguration[]>('configurations'),
  chooseUpdate: () => invoke<string | undefined>('choose_update'),
  launch: (request: LaunchRequest) => invoke<LaunchedCamera>('launch', { request }),
  launchCommand: () => invoke<LaunchedCamera | undefined>('launch_command'),
  launchCli: () => invoke<LaunchedCamera | undefined>('launch_cli'),
  close: () => invoke<void>('close'),
};

export function desktopBridge(): DesktopBridge | undefined {
  return (window as Window & { fujimodDesktop?: DesktopBridge }).fujimodDesktop;
}
