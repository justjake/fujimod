import type { DesktopBridge } from './launch';

export function desktopBridge(): DesktopBridge | undefined {
  return (window as Window & { fujimodDesktop?: DesktopBridge }).fujimodDesktop;
}
