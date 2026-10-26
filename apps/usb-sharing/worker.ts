import { contributions } from './contributions.ts';
export { Contributions } from './contributions.ts';
import type { Env, BudgetRecord, IceServer, Attachment } from './env.ts';
// USB rendezvous, explicit issue-report intake and website error logs. Shared USB payloads stay on WebRTC.
import { issueReports } from './issue-reports.ts';
import { errorLogs } from './error-logs.ts';
export { IssueReports } from './issue-reports.ts';
import { DurableObject } from 'cloudflare:workers';

const ROOM = /^[A-Za-z0-9_-]{43}$/;
const MAX_MESSAGE = 65536;
const CLIENTS = 16;
const PENDING_MS = 10000;
const STUN = [{ urls: ['stun:stun.cloudflare.com:3478'] }];
const fail = (message: string, status: number) => new Response(message, { status });
function integer(value: string | undefined, fallback: number, minimum: number, maximum: number) {
  const number = Number(value ?? fallback);
  if (!Number.isInteger(number) || number < minimum || number > maximum)
    throw Error('Invalid service configuration');
  return number;
}
function send(socket: WebSocket, frame: object) {
  if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(frame));
}
function attachment(socket: WebSocket): Attachment {
  return socket.deserializeAttachment() as Attachment;
}

export default {
  async fetch(request: Request, env: Env) {
    const url = new URL(request.url);
    if (url.pathname === '/contributions' || url.pathname.startsWith('/contributions/')) return contributions(request, env);
    if (url.pathname === '/issue-reports') return issueReports(request, env);
    if (url.pathname === '/error-logs') return errorLogs(request, env);
    if (url.pathname === '/health' && request.method === 'GET')
      return Response.json({ service: 'fujimod-usb-sharing' });
    if (url.pathname !== '/usb-proxy/signal') return fail('Not found', 404);
    if (request.method !== 'GET' || request.headers.get('Upgrade')?.toLowerCase() !== 'websocket')
      return fail('WebSocket upgrade required', 426);
    const origin = request.headers.get('Origin');
    const allowed = (env.ALLOWED_ORIGINS || '').split(',').map((s) => s.trim());
    // Native controllers have no Origin. Origin filtering is not authentication.
    if (origin && !allowed.includes(origin)) return fail('Origin not allowed', 403);
    const room = url.searchParams.get('room');
    if (!room || !ROOM.test(room)) return fail('Invalid room', 400);
    const { success } = await env.CONNECTIONS.limit({
      key: request.headers.get('CF-Connecting-IP') || 'local',
    });
    if (!success) return fail('Connection rate exceeded', 429);
    return env.ROOMS.getByName(room).fetch(request);
  },
};

// A global issuance budget, not a bandwidth/billing cap. Reserve before calling
// the TURN API; failed requests also consume a slot to bound upstream attempts.
export class TurnBudget extends DurableObject<Env> {
  async reserve() {
    const limit = integer(this.env.TURN_DAILY_ISSUE_LIMIT, 50, 1, 100000);
    return this.ctx.storage.transaction(async (storage) => {
      const day = new Date().toISOString().slice(0, 10);
      const previous = await storage.get<BudgetRecord>('issued');
      const count = previous?.day === day ? previous.count : 0;
      if (count >= limit) return false;
      await storage.put('issued', { day, count: count + 1 });
      return true;
    });
  }
}

export class ShareRoom extends DurableObject<Env> {
  sockets() {
    return this.ctx.getWebSockets();
  }
  host() {
    return this.sockets().find((socket) => attachment(socket)?.role === 'host');
  }
  async fetch(request: Request) {
    if (await this.ctx.storage.get('revoked')) return fail('Camera sharing ended', 410);
    if (this.sockets().length >= CLIENTS + 1) return fail('Room is full', 429);
    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    this.ctx.acceptWebSocket(server);
    server.serializeAttachment({
      role: 'pending',
      room: new URL(request.url).searchParams.get('room'),
      accepted: Date.now(),
    });
    await this.schedule();
    return new Response(null, { status: 101, webSocket: client });
  }
  async schedule() {
    const deadlines = this.sockets().flatMap((socket) => {
      const state = attachment(socket);
      if (state?.role === 'closed') return [];
      return state?.role === 'pending'
        ? [state.accepted + PENDING_MS]
        : state?.expires
          ? [state.expires]
          : [];
    });
    if (deadlines.length)
      await this.ctx.storage.setAlarm(Math.max(Date.now() + 1, Math.min(...deadlines)));
    else await this.ctx.storage.deleteAlarm();
  }
  async credentials(ttl: number): Promise<IceServer[]> {
    const keyId = this.env.TURN_KEY_ID,
      apiToken = this.env.TURN_API_TOKEN;
    if (!keyId && !apiToken) return STUN;
    if (!keyId || !apiToken) throw Error('TURN service is not configured');
    if (!(await this.env.TURN_BUDGET.getByName('daily').reserve()))
      throw Error('Daily TURN credential limit reached');
    const response = await fetch(
      `https://rtc.live.cloudflare.com/v1/turn/keys/${encodeURIComponent(keyId)}/credentials/generate-ice-servers`,
      {
        method: 'POST',
        headers: { Authorization: `Bearer ${apiToken}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ ttl }),
        signal: AbortSignal.timeout(8000),
      },
    );
    if (!response.ok) throw Error('TURN credential service unavailable');
    const result = await response.json<{ iceServers: IceServer[] }>();
    if (
      !Array.isArray(result.iceServers) ||
      !result.iceServers.some((server) => server.username && server.credential)
    )
      throw Error('Invalid TURN credential response');
    return result.iceServers;
  }
  async webSocketMessage(socket: WebSocket, message: string | ArrayBuffer) {
    try {
      if (typeof message !== 'string' || message.length > MAX_MESSAGE)
        throw Error('Invalid signaling message');
      const frame: {
          type: string;
          room: string;
          role: 'host' | 'client';
          to: string;
          data: string;
        } = JSON.parse(message),
        state = attachment(socket);
      if (!state || state.role === 'closed') throw Error('Connection closed');
      const now = Date.now();
      if (now - (state.window || 0) >= 60000) {
        state.window = now;
        state.messages = 0;
      }
      if ((state.messages = (state.messages ?? 0) + 1) > 60) throw Error('Signaling rate exceeded');
      socket.serializeAttachment(state);
      if (state.role === 'pending') {
        if (
          frame.type !== 'join' ||
          frame.room !== state.room ||
          !['host', 'client'].includes(frame.role)
        )
          throw Error('Invalid room handshake');
        if (await this.ctx.storage.get('revoked')) throw Error('Camera sharing ended');
        if (frame.role === 'host') {
          if (
            this.sockets().some((peer) =>
              ['host', 'preparing'].includes(attachment(peer)?.role ?? ''),
            )
          )
            throw Error('Room already has an owner');
          const ttl = integer(this.env.TURN_TTL_SECONDS, 86400, 60, 86400);
          state.role = 'preparing';
          state.peer = 'host';
          state.expires = now + ttl * 1000;
          socket.serializeAttachment(state); // Reserve ownership before asynchronous issuance.
          const iceServers = await this.credentials(ttl);
          if (socket.readyState !== WebSocket.OPEN) return;
          if (JSON.stringify(iceServers).length > 1500)
            throw Error('TURN credential response too large');
          state.role = 'host';
          state.iceServers = iceServers;
          socket.serializeAttachment(state);
          send(socket, { type: 'ready', peer: 'host', ice_servers: iceServers });
        } else {
          const host = this.host();
          if (!host) throw Error('Camera share unavailable');
          state.role = 'client';
          state.peer = crypto.randomUUID();
          socket.serializeAttachment(state);
          send(host, { type: 'joined', peer: state.peer });
          send(socket, {
            type: 'ready',
            peer: state.peer,
            ice_servers: attachment(host).iceServers,
          });
        }
        await this.schedule();
        return;
      }
      if (
        frame.type !== 'signal' ||
        typeof frame.data !== 'string' ||
        frame.data.length > 60000 ||
        !/^[A-Za-z0-9_-]+$/.test(frame.data)
      )
        throw Error('Invalid signaling frame');
      const target =
        state.role === 'host'
          ? this.sockets().find(
              (peer) => attachment(peer)?.role === 'client' && attachment(peer).peer === frame.to,
            )
          : this.host();
      if (target) send(target, { type: 'signal', from: state.peer, data: frame.data });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      send(socket, {
        type: 'error',
        message:
          message === 'Invalid service configuration' ? 'Sharing service unavailable' : message,
      });
      socket.close(1008, 'Sharing request rejected');
      await this.remove(socket);
    }
  }
  async remove(socket: WebSocket) {
    const state = attachment(socket);
    if (!state || state.role === 'closed') return;
    socket.serializeAttachment({ ...state, role: 'closed' });
    if (['host', 'preparing'].includes(state.role)) {
      await this.ctx.storage.put('revoked', true);
      for (const peer of this.sockets()) {
        if (peer === socket || attachment(peer)?.role === 'closed') continue;
        send(peer, { type: 'ended' });
        peer.close(1000, 'Camera sharing ended');
        peer.serializeAttachment({ ...attachment(peer), role: 'closed' });
      }
      // End the room immediately, then revoke its shared temporary relay login.
      const users = new Set(
        (state.iceServers || []).flatMap((server) => (server.username ? [server.username] : [])),
      );
      for (const user of users) {
        try {
          await fetch(
            `https://rtc.live.cloudflare.com/v1/turn/keys/${encodeURIComponent(this.env.TURN_KEY_ID ?? '')}/credentials/${encodeURIComponent(user)}/revoke`,
            {
              method: 'POST',
              headers: { Authorization: `Bearer ${this.env.TURN_API_TOKEN}` },
              signal: AbortSignal.timeout(3000),
            },
          );
        } catch {
          /* The temporary credentials still expire at their original TTL. */
        }
      }
    } else if (state.role === 'client') {
      const host = this.host();
      if (host) send(host, { type: 'left', peer: state.peer });
    }
    await this.schedule();
  }
  async webSocketClose(socket: WebSocket, _code: number) {
    await this.remove(socket);
    socket.close(1000);
  }
  async webSocketError(socket: WebSocket) {
    await this.remove(socket);
    socket.close(1011, 'Connection failed');
  }
  async alarm() {
    const now = Date.now();
    for (const socket of this.sockets()) {
      const state = attachment(socket);
      if (
        (state?.role === 'pending' && now >= state.accepted + PENDING_MS) ||
        (['host', 'preparing'].includes(state?.role ?? '') && state.expires && now >= state.expires)
      ) {
        send(socket, { type: 'ended' });
        socket.close(1000, 'Sharing session expired');
        await this.remove(socket);
      }
    }
    await this.schedule();
  }
}
