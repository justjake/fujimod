import assert from 'node:assert/strict';
import test from 'node:test';
import { gather } from './peer.ts';

class Connection extends EventTarget {
  iceGatheringState: RTCIceGatheringState;
  localDescription: RTCSessionDescription;
  constructor(sdp: string) {
    super();
    this.iceGatheringState = 'gathering';
    this.localDescription = { sdp, type: 'offer', toJSON: () => ({ sdp, type: 'offer' }) };
  }
}
const host = 'v=0\r\na=candidate:1 1 udp 1 10.0.0.1 5000 typ host\r\n';

await test('gathering that never completes sends the candidates found so far', async () => {
  await gather(new Connection(host), { settle: 10, limit: 100 });
});

await test('gathering with no candidates fails at the limit', async () => {
  await assert.rejects(
    gather(new Connection('v=0\r\n'), { settle: 10, limit: 50 }),
    /no candidates/,
  );
});

await test('completed gathering resolves at once', async () => {
  const connection = new Connection('v=0\r\n');
  setTimeout(() => {
    connection.iceGatheringState = 'complete';
    connection.dispatchEvent(new Event('icegatheringstatechange'));
  }, 5);
  await gather(connection, { settle: 1000, limit: 2000 });
});
