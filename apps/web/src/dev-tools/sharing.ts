import {
  ShareHost,
  newTicket,
  shareUrl,
  parseShare,
} from '../../../../crates/usb-proxy/web/peer.ts';
export { ShareHost, newTicket, shareUrl };
export function sharingConfigured(): boolean {
  return (
    Boolean(__FUJIMOD_SIGNAL_URL__) ||
    import.meta.env.DEV ||
    import.meta.env.MODE === 'local-preview'
  );
}
export function remoteControlUrl(current: string): boolean {
  const fragment = new URLSearchParams(new URL(current).hash.slice(1).replace(/^\?/, ''));
  if (!fragment.has('remotecontrol')) return false;
  parseShare(current);
  return true;
}
export function signalUrl(current: string): string {
  if (!sharingConfigured()) throw Error('USB sharing service is not configured');
  const url = new URL(__FUJIMOD_SIGNAL_URL__ || '/usb-proxy/signal', current);
  url.protocol =
    url.protocol === 'https:' ? 'wss:' : url.protocol === 'http:' ? 'ws:' : url.protocol;
  return url.href;
}
declare const __FUJIMOD_SIGNAL_URL__: string;
export function useShareSheet(): boolean {
  return typeof navigator.share === 'function' && navigator.maxTouchPoints > 0;
}
