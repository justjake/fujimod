import type { Miniflare, WebSocket } from 'miniflare';
interface TestFrame {
  type: string;
  peer?: string;
  ice_servers?: unknown[];
  message?: string;
  from?: string;
  data?: string;
}
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runtime } from './test-runtime.ts';
import { setTimeout as delay } from 'node:timers/promises';

const origin = 'https://justjake.github.io';
const room = (byte = 1) => Buffer.alloc(32, byte).toString('base64url');

function inbox(socket: WebSocket) {
  const queue: TestFrame[] = [],
    waiting: { type: string; resolve: (frame: TestFrame) => void }[] = [];
  socket.addEventListener('message', ({ data }) => {
    assert.equal(typeof data, 'string');
    if (typeof data !== 'string') throw Error('Expected signaling text');
    const frame: TestFrame = JSON.parse(data);
    const match = waiting.findIndex((wait) => wait.type === frame.type || frame.type === 'error');
    if (match >= 0) waiting.splice(match, 1)[0].resolve(frame);
    else queue.push(frame);
  });
  return async (type: string) => {
    const match = queue.findIndex((frame) => frame.type === type || frame.type === 'error');
    if (match >= 0) return queue.splice(match, 1)[0];
    let pending!: { type: string; resolve: (frame: TestFrame) => void };
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([
        new Promise<TestFrame>((resolve) => {
          pending = { type, resolve };
          waiting.push(pending);
        }),
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => reject(Error(`No ${type} frame`)), 5000);
        }),
      ]);
    } finally {
      clearTimeout(timer);
      const index = waiting.indexOf(pending);
      if (index >= 0) waiting.splice(index, 1);
    }
  };
}
async function connect(mf: Miniflare, id: string, role: string, native = false, ownerKey?: string) {
  const response = await mf.dispatchFetch(`https://worker.example/usb-proxy/signal?room=${id}`, {
    headers: { Upgrade: 'websocket', ...(native ? {} : { Origin: origin }) },
  });
  assert.equal(response.status, 101);
  const socket = response.webSocket;
  assert.ok(socket);
  const next = inbox(socket);
  socket.accept();
  socket.send(
    JSON.stringify({ type: 'join', room: id, role, ...(ownerKey ? { owner_key: ownerKey } : {}) }),
  );
  return {
    socket,
    next,
    ready: await next('ready'),
    send: (frame: object) => socket.send(JSON.stringify(frame)),
  };
}
async function until(predicate: () => boolean | Promise<boolean>) {
  for (let i = 0; i < 100; i++) {
    if (await predicate()) return;
    await delay(10);
  }
  assert.fail('condition not reached');
}

await test('real Worker routes encrypted signaling to assigned peers and revokes owner rooms', async () => {
  const mf = await runtime();
  try {
    const health = await mf.dispatchFetch('https://worker.example/health');
    assert.deepEqual(await health.json(), { service: 'fujimod-usb-sharing' });
    const badOrigin = await mf.dispatchFetch(
      `https://worker.example/usb-proxy/signal?room=${room()}`,
      { headers: { Upgrade: 'websocket', Origin: 'https://untrusted.example' } },
    );
    assert.equal(badOrigin.status, 403);
    assert.equal((await mf.dispatchFetch('https://worker.example/usb-proxy/signal')).status, 426);
    const owner = await connect(mf, room(), 'host');
    assert.equal(owner.ready.peer, 'host');
    assert.deepEqual(owner.ready.ice_servers, [{ urls: ['stun:stun.cloudflare.com:3478'] }]);
    const client = await connect(mf, room(), 'client', true);
    assert.equal((await owner.next('joined')).peer, client.ready.peer);
    const duplicate = await connect(mf, room(), 'host');
    assert.match(duplicate.ready.message ?? '', /already has an owner/);
    client.send({ type: 'signal', from: 'forged', data: 'ZW5jcnlwdGVk' });
    assert.deepEqual(await owner.next('signal'), {
      type: 'signal',
      from: client.ready.peer,
      data: 'ZW5jcnlwdGVk',
    });
    owner.send({ type: 'signal', to: client.ready.peer, data: 'YW5zd2Vy' });
    assert.deepEqual(await client.next('signal'), {
      type: 'signal',
      from: 'host',
      data: 'YW5zd2Vy',
    });
    const observer = await connect(mf, room(), 'client');
    observer.socket.close(1000);
    assert.equal((await owner.next('left')).peer, observer.ready.peer);
    owner.socket.close(1000);
    assert.equal((await client.next('ended')).type, 'ended');
    await until(
      async () =>
        (
          await mf.dispatchFetch(`https://worker.example/usb-proxy/signal?room=${room()}`, {
            headers: { Upgrade: 'websocket' },
          })
        ).status === 410,
    );
  } finally {
    await mf.dispose();
  }
});

await test('master secret stays server-side; one temporary credential per room is budgeted and revoked', async () => {
  const requests: { url: string; body: string }[] = [];
  const ice = [
    {
      urls: ['turn:turn.cloudflare.com:3478?transport=udp'],
      username: 'temporary-user',
      credential: 'temporary-password',
    },
  ];
  const mf = await runtime({
    bindings: {
      TURN_KEY_ID: 'key-id',
      TURN_API_TOKEN: 'MASTER-SECRET',
      TURN_DAILY_ISSUE_LIMIT: '1',
      TURN_TTL_SECONDS: '3600',
    },
    outboundService: async (request) => {
      assert.equal(request.headers.get('Authorization'), 'Bearer MASTER-SECRET');
      requests.push({ url: request.url, body: await request.text() });
      if (request.url.endsWith('/revoke')) return new Response(null, { status: 204 });
      return Response.json({ iceServers: ice });
    },
  });
  try {
    const owner = await connect(mf, room(), 'host');
    assert.deepEqual(owner.ready.ice_servers, ice);
    assert.equal(JSON.stringify(owner.ready).includes('MASTER-SECRET'), false);
    assert.equal(JSON.stringify(owner.ready).includes('key-id'), false);
    assert.deepEqual(JSON.parse(requests[0].body), { ttl: 3600 });
    const client = await connect(mf, room(), 'client');
    assert.deepEqual(client.ready.ice_servers, ice);
    assert.equal(requests.length, 1);
    const denied = await connect(mf, room(2), 'host');
    assert.match(denied.ready.message ?? '', /Daily TURN credential limit/);
    assert.equal(requests.length, 1);
    owner.socket.close(1000);
    assert.equal((await client.next('ended')).type, 'ended');
    await until(() =>
      requests.some((request) => request.url.endsWith('/credentials/temporary-user/revoke')),
    );
  } finally {
    await mf.dispose();
  }
});

await test('invalid frames and partial credential configuration fail closed', async () => {
  const mf = await runtime({ bindings: { TURN_KEY_ID: 'missing-token' } });
  try {
    const owner = await connect(mf, room(), 'host');
    assert.match(owner.ready.message ?? '', /not configured/);
  } finally {
    await mf.dispose();
  }
  const direct = await runtime();
  try {
    const owner = await connect(direct, room(), 'host');
    const client = await connect(direct, room(), 'client');
    client.send({ type: 'signal', data: '!' });
    assert.match((await client.next('error')).message ?? '', /Invalid signaling frame/);
    assert.equal((await owner.next('left')).peer, client.ready.peer);
    owner.socket.close(1000);
  } finally {
    await direct.dispose();
  }
});

await test('persistent owner key is bound on first join and authorizes offline recovery', async () => {
  const mf = await runtime();
  const id = room(9);
  const ownerKey = Buffer.alloc(32, 73).toString('base64url');
  const wrongKey = Buffer.alloc(32, 74).toString('base64url');
  try {
    const owner = await connect(mf, id, 'host', false, ownerKey);
    assert.equal(owner.ready.peer, 'host');
    assert.deepEqual(owner.ready.ice_servers, [{ urls: ['stun:stun.cloudflare.com:3478'] }]);
    const client = await connect(mf, id, 'client');

    owner.socket.close(1000);
    assert.equal((await client.next('host_offline')).type, 'host_offline');

    const offlineClient = await connect(mf, id, 'client');
    assert.match(offlineClient.ready.message ?? '', /Camera share offline/);

    const wrongOwner = await connect(mf, id, 'host', false, wrongKey);
    assert.match(wrongOwner.ready.message ?? '', /Invalid owner key/);
    const missingOwnerKey = await connect(mf, id, 'host');
    assert.match(missingOwnerKey.ready.message ?? '', /Owner key required/);

    const resumedOwner = await connect(mf, id, 'host', false, ownerKey);
    assert.equal(resumedOwner.ready.peer, 'host');
    assert.deepEqual(resumedOwner.ready.ice_servers, [{ urls: ['stun:stun.cloudflare.com:3478'] }]);
    const resumedClient = await connect(mf, id, 'client');
    assert.equal(resumedClient.ready.peer?.length, 36);

    resumedOwner.send({ type: 'end' });
    assert.equal((await resumedOwner.next('ended')).type, 'ended');
    assert.equal((await resumedClient.next('ended')).type, 'ended');
    await until(
      async () =>
        (
          await mf.dispatchFetch(`https://worker.example/usb-proxy/signal?room=${id}`, {
            headers: { Upgrade: 'websocket' },
          })
        ).status === 410,
    );

    const ordinaryId = room(11);
    const ordinaryOwner = await connect(mf, ordinaryId, 'host');
    const keyedOrdinaryOwner = await connect(mf, ordinaryId, 'host', false, ownerKey);
    assert.match(keyedOrdinaryOwner.ready.message ?? '', /not valid for this room/);
    ordinaryOwner.socket.close(1000);
  } finally {
    await mf.dispose();
  }
});

await test(
  'a persistent owner can replace a stale socket with the same key',
  { timeout: 10000 },
  async () => {
    const mf = await runtime();
    const id = room(12);
    const key = room(76);
    try {
      const owner = await connect(mf, id, 'host', false, key);
      assert.equal(owner.ready.peer, 'host');
      const client = await connect(mf, id, 'client');
      const wrongOwner = await connect(mf, id, 'host', false, room(77));
      assert.match(wrongOwner.ready.message ?? '', /Invalid owner key/);

      const closed = [owner.socket, client.socket].map(
        (socket) =>
          new Promise<number>((resolve) =>
            socket.addEventListener('close', (event) => resolve(event.code)),
          ),
      );
      const resumed = await connect(mf, id, 'host', false, key);
      assert.equal(resumed.ready.peer, 'host');
      assert.deepEqual(await Promise.all(closed), [1012, 1012]);

      const receiver = await connect(mf, id, 'client');
      receiver.send({ type: 'signal', data: 'b2ZmZXI' });
      assert.deepEqual(await resumed.next('signal'), {
        type: 'signal',
        from: receiver.ready.peer,
        data: 'b2ZmZXI',
      });
    } finally {
      await mf.dispose();
    }
  },
);

await test('persistent room idle alarm revokes after SHARE_IDLE_SECONDS', async () => {
  const mf = await runtime({ bindings: { SHARE_IDLE_SECONDS: '1' } });
  const id = room(10);
  const ownerKey = Buffer.alloc(32, 75).toString('base64url');
  try {
    const owner = await connect(mf, id, 'host', false, ownerKey);
    owner.socket.close(1000);
    await delay(1200);
    await until(async () => {
      const response = await mf.dispatchFetch(
        `https://worker.example/usb-proxy/signal?room=${id}`,
        { headers: { Upgrade: 'websocket' } },
      );
      if (response.status === 101) response.webSocket?.close(1000);
      return response.status === 410;
    });
  } finally {
    await mf.dispose();
  }
});
