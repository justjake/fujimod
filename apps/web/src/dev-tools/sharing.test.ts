import { expect, test } from 'vite-plus/test';
import { newTicket, parseShare, shareUrl } from '../../../../crates/usb-proxy/web/peer.ts';
import { remoteControlUrl } from './sharing';
test('share capability stays in the fragment and preserves the current page and query', () => {
  const ticket = newTicket('wss://signal.example/usb-proxy/signal', []);
  const link = shareUrl(
    'https://example.com/dev-tools/?firmware=body-b&target=simulator#features',
    ticket,
  );
  expect(
    link.startsWith(
      'https://example.com/dev-tools/?firmware=body-b&target=simulator#?remotecontrol=',
    ),
  ).toBe(true);
  expect(parseShare(link)).toEqual(ticket);
  expect(new URL(link).search).not.toContain(ticket.key);
  expect(remoteControlUrl(link)).toBe(true);
  expect(remoteControlUrl('https://example.com/dev-tools/#features')).toBe(false);
  expect(ticket.room).toHaveLength(43);
  expect(ticket.key).toHaveLength(43);
  expect(newTicket(ticket.signal, []).key).not.toBe(ticket.key);
});
test('malformed share links fail instead of selecting a local camera', () => {
  for (const value of ['', 'bad', 'e30'])
    expect(() => remoteControlUrl(`https://example.com/#?remotecontrol=${value}`)).toThrow();
});

test('ambiguous tickets and signaling credentials are rejected', () => {
  const ticket = newTicket('wss://signal.example/usb-proxy/signal', []);
  const link = shareUrl('https://example.com/dev-tools/', ticket);
  expect(() => parseShare(link + '&remotecontrol=e30')).toThrow();
  expect(() =>
    parseShare(shareUrl(link, { ...ticket, signal: 'wss://user:secret@signal.example/' })),
  ).toThrow();
});
