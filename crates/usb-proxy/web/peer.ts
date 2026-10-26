import type {
  Ticket,
  UsbExporter,
  Callbacks,
  ShareStatus,
  Descriptor,
  SignalFrame,
} from './types.ts';
export type {
  Ticket,
  UsbExporter,
  Callbacks,
  ShareStatus,
  Descriptor,
  Operation,
  Completion,
} from './types.ts';
export type { ShareTrafficEvent } from './share-traffic.ts';
import { VERSION, MAX_MESSAGE, encodeMessage, decodeMessage } from './wire.ts';
import { observeRequest } from './share-traffic.ts';
import { Pipeline } from './pipeline.ts';
import type { ExecuteMessage, Completion as UsbCompletion } from './types.ts';
// WebRTC USB transport. Only encrypted SDP crosses the rendezvous service.
const MAX = MAX_MESSAGE;
const encoder = new TextEncoder();
export const encodeUrl = (bytes: Uint8Array) => {
  let text = '';
  for (let i = 0; i < bytes.length; i += 32768)
    text += String.fromCharCode(...bytes.subarray(i, i + 32768));
  return btoa(text).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '');
};
const decodeUrl = (text: string) =>
  Uint8Array.from(atob(text.replaceAll('-', '+').replaceAll('_', '/')), (c) => c.charCodeAt(0));
const random = () => encodeUrl(crypto.getRandomValues(new Uint8Array(32)));
export function newTicket(signal: string, stun = ['stun:stun.cloudflare.com:3478']): Ticket {
  return { v: 1, signal, room: random(), key: random(), stun };
}
export function parseShare(url: string): Ticket {
  const values = new URLSearchParams(new URL(url).hash.slice(1).replace(/^\?/, '')).getAll(
    'remotecontrol',
  );
  const [value] = values;
  if (values.length !== 1 || !value || value.length > 4096)
    throw Error('Invalid camera share link');
  const ticket: Ticket = JSON.parse(new TextDecoder().decode(decodeUrl(value)));
  const signal = new URL(ticket.signal);
  if (
    ticket.v !== 1 ||
    decodeUrl(ticket.key).length !== 32 ||
    decodeUrl(ticket.room).length !== 32 ||
    !/^wss?:$/.test(signal.protocol) ||
    signal.username ||
    signal.password ||
    !Array.isArray(ticket.stun) ||
    ticket.stun.length > 4 ||
    ticket.stun.some(
      (server) => typeof server !== 'string' || !server.startsWith('stun:') || server.length > 256,
    )
  )
    throw Error('Invalid camera share link');
  return ticket;
}
export function shareUrl(current: string, ticket: Ticket) {
  const url = new URL(current);
  url.hash = '?remotecontrol=' + encodeUrl(encoder.encode(JSON.stringify(ticket)));
  return url.href;
}
async function cryptKey(ticket: Ticket) {
  return crypto.subtle.importKey('raw', decodeUrl(ticket.key), 'AES-GCM', false, [
    'encrypt',
    'decrypt',
  ]);
}
async function seal(ticket: Ticket, peer: string, value: object) {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const data = new Uint8Array(
    await crypto.subtle.encrypt(
      { name: 'AES-GCM', iv, additionalData: encoder.encode(ticket.room + '/' + peer) },
      await cryptKey(ticket),
      encoder.encode(JSON.stringify(value)),
    ),
  );
  const result = new Uint8Array(12 + data.length);
  result.set(iv);
  result.set(data, 12);
  return encodeUrl(result);
}
async function unseal(
  ticket: Ticket,
  peer: string,
  value: string,
): Promise<RTCSessionDescriptionInit | { type: 'error'; message: string }> {
  const bytes = decodeUrl(value);
  const data = await crypto.subtle.decrypt(
    {
      name: 'AES-GCM',
      iv: bytes.slice(0, 12),
      additionalData: encoder.encode(ticket.room + '/' + peer),
    },
    await cryptKey(ticket),
    bytes.slice(12),
  );
  return JSON.parse(new TextDecoder().decode(data));
}
function signaling(
  ticket: Ticket,
  role: string,
  message: (frame: SignalFrame) => void | Promise<void>,
  ended: (error: Error) => void,
) {
  const url = new URL(ticket.signal);
  url.searchParams.set('room', ticket.room);
  const socket = new WebSocket(url);
  const ready = new Promise<SignalFrame>((resolve, reject) => {
    const timer = setTimeout(() => {
      socket.close();
      reject(Error('Signaling connection timed out'));
    }, 10000);
    socket.onopen = () => socket.send(JSON.stringify({ type: 'join', room: ticket.room, role }));
    socket.onmessage = ({ data }) => {
      const frame: SignalFrame = JSON.parse(data);
      if (frame.type === 'ready') {
        clearTimeout(timer);
        resolve(frame);
      } else if (frame.type === 'error') {
        clearTimeout(timer);
        const error = Error(frame.message || 'Camera share unavailable');
        reject(error);
        ended(error);
        socket.close();
      } else if (frame.type === 'ended') {
        socket.close();
        ended(Error('Camera sharing ended'));
      } else Promise.resolve(message(frame)).catch(ended);
    };
    socket.onerror = () => {
      clearTimeout(timer);
      reject(Error('Signaling service unavailable'));
    };
    socket.onclose = () => {
      clearTimeout(timer);
      reject(Error('Camera share unavailable'));
      ended(Error('Signaling connection closed'));
    };
  });
  return {
    ready,
    send: (frame: object) => socket.send(JSON.stringify(frame)),
    close: () => socket.close(),
  };
}
// Connect and disconnect reasons are display text from the controller.
function reason(value: unknown) {
  return typeof value === 'string' ? value.slice(0, 512) : '';
}
export async function gather(
  pc: Pick<
    RTCPeerConnection,
    'iceGatheringState' | 'localDescription' | 'addEventListener' | 'removeEventListener'
  >,
  { settle = 3000, limit = 12000 } = {},
) {
  if (pc.iceGatheringState === 'complete') return;
  // Some browsers leave gathering open when one interface or server never
  // answers. After `settle`, send the candidates found so far; fail only if
  // none exist by `limit`.
  const found = () => /^a=candidate:/m.test(pc.localDescription?.sdp || '');
  await new Promise<void>((resolve, reject) => {
    const finish = (error?: Error) => {
      clearTimeout(settled);
      clearTimeout(expired);
      pc.removeEventListener('icegatheringstatechange', changed);
      if (error) reject(error);
      else resolve();
    };
    const settled = setTimeout(() => found() && finish(), settle);
    const expired = setTimeout(
      () => finish(found() ? undefined : Error('ICE gathering found no candidates')),
      limit,
    );
    function changed() {
      if (pc.iceGatheringState === 'complete') finish();
    }
    pc.addEventListener('icegatheringstatechange', changed);
  });
}
// Ordered, bounded framing handles USB requests larger than a single SCTP message.
export class PeerChannel {
  channel: RTCDataChannel;
  pc: RTCPeerConnection;
  queue: Uint8Array[];
  waiters: { resolve: (bytes: Uint8Array) => void; reject: (error: Error) => void }[];
  parts: Uint8Array[];
  size: number;
  queued = 0;
  ended: boolean;
  onclose?: () => void;
  constructor(channel: RTCDataChannel, pc: RTCPeerConnection) {
    this.channel = channel;
    this.pc = pc;
    this.queue = [];
    this.waiters = [];
    this.parts = [];
    this.size = 0;
    this.ended = false;
    channel.binaryType = 'arraybuffer';
    channel.bufferedAmountLowThreshold = 131072;
    channel.onmessage = ({ data }) => {
      const chunk = new Uint8Array(data);
      if (chunk.length < 1 || chunk[0] > 1 || (this.size += chunk.length - 1) > MAX) {
        this.close();
        return;
      }
      this.parts.push(chunk.subarray(1));
      if (chunk[0] === 1) {
        const bytes = new Uint8Array(this.size);
        let offset = 0;
        for (const part of this.parts) {
          bytes.set(part, offset);
          offset += part.length;
        }
        this.parts = [];
        this.size = 0;
        const waiter = this.waiters.shift();
        if (waiter) waiter.resolve(bytes);
        else {
          this.queue.push(bytes);
          this.queued += bytes.length;
        }
        // A pipelining controller keeps up to 4096 operations and 1 MiB (or
        // one larger transfer) in flight; anything beyond that is a fault.
        if (this.queue.length > 16384 || this.queued > 2 * MAX) this.close();
      }
    };
    channel.onclose = () => this.close();
    channel.onerror = () => this.close();
    pc.addEventListener('connectionstatechange', () => {
      if (['failed', 'closed'].includes(pc.connectionState)) this.close();
    });
  }
  async send(bytes: Uint8Array) {
    if (!(bytes instanceof Uint8Array)) throw Error('Expected binary peer frame');
    if (bytes.length > MAX) throw Error('USB message exceeds limit');
    for (let offset = 0; offset < bytes.length; offset += 16000) {
      if (this.channel.bufferedAmount > 262144)
        await new Promise<void>((resolve, reject) => {
          const low = () => {
              cleanup();
              resolve();
            },
            close = () => {
              cleanup();
              reject(Error('Peer disconnected; completion unknown'));
            };
          const timer = setTimeout(close, 10000);
          const cleanup = () => {
            clearTimeout(timer);
            this.channel.removeEventListener('bufferedamountlow', low);
            this.channel.removeEventListener('close', close);
          };
          this.channel.addEventListener('bufferedamountlow', low, { once: true });
          this.channel.addEventListener('close', close, { once: true });
        });
      if (this.ended || this.channel.readyState !== 'open')
        throw Error('Peer disconnected; completion unknown');
      const piece = bytes.subarray(offset, offset + 16000),
        packet = new Uint8Array(piece.length + 1);
      packet[0] = offset + piece.length === bytes.length ? 1 : 0;
      packet.set(piece, 1);
      this.channel.send(packet);
    }
  }
  receive(): Promise<Uint8Array> {
    if (this.queue.length) {
      const bytes = this.queue.shift()!;
      this.queued -= bytes.length;
      return Promise.resolve(bytes);
    }
    if (this.ended) return Promise.reject(Error('Peer disconnected; completion unknown'));
    return new Promise<Uint8Array>((resolve, reject) => this.waiters.push({ resolve, reject }));
  }
  close() {
    if (this.ended) return;
    this.ended = true;
    for (const waiter of this.waiters.splice(0))
      waiter.reject(Error('Peer disconnected; completion unknown'));
    this.parts = [];
    this.queue = [];
    this.queued = 0;
    this.size = 0;
    this.channel.close();
    this.pc.close();
    this.onclose?.();
  }
}
function configuration(stun: string[]) {
  return { iceServers: stun.map((urls) => ({ urls })) };
}
export async function connectPeer(url: string) {
  const ticket = parseShare(url),
    pc = new RTCPeerConnection(configuration(ticket.stun || []));
  const channel = pc.createDataChannel('fujimod-usb', { ordered: true });
  const peer = new PeerChannel(channel, pc);
  let rejectOpen: (error: Error) => void = () => {};
  const opened = new Promise<void>((resolve, reject) => {
    rejectOpen = reject;
    channel.onopen = () => resolve();
  });
  opened.catch(() => {});
  const signal = signaling(
    ticket,
    'client',
    async (frame) => {
      if (frame.type !== 'signal' || frame.from !== 'host') return;
      const answer = await unseal(ticket, id, frame.data);
      if (answer.type === 'error') throw Error(`Share owner could not connect: ${answer.message}`);
      if (answer.type !== 'answer') throw Error('Invalid peer answer');
      await pc.setRemoteDescription(answer);
    },
    (error) => {
      if (channel.readyState !== 'open') {
        rejectOpen(error);
        peer.close();
      }
    },
  );
  const timer = setTimeout(() => {
    rejectOpen(Error('Direct connection unavailable'));
    peer.close();
  }, 25000);
  let id = '';
  try {
    const ready = await signal.ready;
    id = ready.peer;
    if (ready.ice_servers) pc.setConfiguration({ iceServers: ready.ice_servers });
    await pc.setLocalDescription(await pc.createOffer());
    await gather(pc);
    signal.send({ type: 'signal', data: await seal(ticket, id, pc.localDescription!.toJSON()) });
    await opened;
    peer.onclose = () => signal.close();
    return peer;
  } catch (error) {
    signal.close();
    peer.close();
    throw error;
  } finally {
    clearTimeout(timer);
  }
}
export class ShareHost {
  ticket: Ticket;
  device: UsbExporter | undefined;
  descriptor: Descriptor;
  offline: string | undefined;
  battery: { level: string; at: number } | undefined;
  callbacks: Callbacks;
  peers: Map<string, { close(): void }>;
  closed: boolean;
  iceServers: RTCIceServer[] = [];
  signal: ReturnType<typeof signaling>;
  static async create(ticket: Ticket, device: UsbExporter, callbacks: Callbacks = {}) {
    const host = new ShareHost(ticket, device, callbacks);
    try {
      const ready = await host.signal.ready;
      host.iceServers = ready.ice_servers || configuration(ticket.stun || []).iceServers;
      return host;
    } catch (error) {
      host.stop();
      throw error;
    }
  }
  constructor(ticket: Ticket, device: UsbExporter, callbacks: Callbacks) {
    this.ticket = ticket;
    this.device = device;
    // The link outlives the camera: while it is away, controllers see it offline.
    this.descriptor = JSON.parse(device.inventoryJson)[0] as Descriptor;
    this.offline = undefined;
    this.battery = undefined;
    this.callbacks = callbacks;
    this.peers = new Map();
    this.closed = false;
    this.signal = signaling(
      ticket,
      'host',
      (frame) =>
        this.message(frame).catch(async (error) => {
          this.peers.get(frame.from)?.close();
          this.peers.delete(frame.from);
          // Report a failed connection on both ends instead of leaving the
          // controller to time out waiting for an answer.
          const reason = error instanceof Error ? error.message : String(error);
          if (!this.closed)
            this.callbacks.error?.(Error(`Remote controller could not connect: ${reason}`));
          if (frame.type !== 'signal' || this.closed) return;
          try {
            this.signal.send({
              type: 'signal',
              to: frame.from,
              data: await seal(this.ticket, frame.from, { type: 'error', message: reason }),
            });
          } catch {
            /* The controller still reports the last step it reached. */
          }
        }),
      (error) => {
        if (!this.closed) {
          this.callbacks.error?.(error);
          this.stop();
        }
      },
    );
  }
  /** Online state and last controller battery report, as controllers see it. */
  status() {
    const fenced = this.device?.fenced(this.descriptor.id);
    const status: ShareStatus = { online: !this.offline && !fenced };
    if (this.offline || fenced) status.reason = this.offline ?? 'not responding';
    if (this.battery) {
      status.battery = this.battery.level;
      status.battery_age_ms = Math.max(0, Date.now() - this.battery.at);
    }
    return status;
  }
  /** The camera went away; keep the link and report it offline with `reason`. */
  cameraLost(reason: string) {
    if (this.closed || !this.device) return;
    const device = this.device;
    this.device = undefined;
    this.offline = reason;
    for (const peer of this.peers.values()) peer.close();
    device.free();
    this.callbacks.state?.(this.status());
  }
  /** The same camera is back (the caller checked its identity); share it again. */
  cameraReturned(device: UsbExporter) {
    if (this.closed) return;
    this.device = device;
    this.descriptor = JSON.parse(device.inventoryJson)[0] as Descriptor;
    this.offline = undefined;
    this.callbacks.state?.(this.status());
  }
  /** One operation for controller `from`, under the share's ownership rules. */
  async run(from: string, peer: PeerChannel, message: ExecuteMessage): Promise<UsbCompletion> {
    const op = message.op;
    if (op.op === 'probe') return {}; /* Network echo; never touches USB. */
    if (op.op === 'list') return { devices: [{ ...this.descriptor, status: this.status() }] };
    if (!this.device) throw Error(`Shared camera offline: ${this.offline}`);
    if (message.device !== this.descriptor.id) throw Error('Unknown shared device');
    if (op.op === 'owner_status' || op.op === 'owner_attention') {
      if (!this.device.owns(from, this.descriptor.id)) throw Error('Shared camera is not claimed');
      const text = op.message;
      if (typeof text !== 'string' || !text.length || new TextEncoder().encode(text).length > 512)
        throw Error('Invalid owner message');
      if (op.op === 'owner_status') {
        if (!this.callbacks.toolStatus) throw Error('Owner log unavailable');
        this.callbacks.toolStatus(from, text);
      } else {
        if (!this.callbacks.countdown) throw Error('Owner countdown unavailable');
        await this.callbacks.countdown(from, text);
        if (
          peer.ended ||
          this.closed ||
          !this.device ||
          !this.device.owns(from, this.descriptor.id)
        )
          throw Error('Owner countdown interrupted');
      }
      return {};
    }
    const reply = decodeMessage(await this.device.execute(from, encodeMessage(message)));
    if (reply.type !== 'result' || reply.seq !== message.seq)
      throw Error('Invalid Rust exporter reply');
    const completion = reply.completion;
    return completion;
  }
  async message(frame: SignalFrame) {
    if (this.closed) return;
    if (frame.type === 'left') {
      this.peers.get(frame.peer)?.close();
      return;
    }
    if (frame.type !== 'signal' || this.peers.has(frame.from)) return;
    const offer = await unseal(this.ticket, frame.from, frame.data);
    if (offer.type !== 'offer') throw Error('Invalid peer offer');
    const pc = new RTCPeerConnection({ iceServers: this.iceServers });
    this.peers.set(frame.from, { close: () => pc.close() });
    const timer = setTimeout(() => {
      if (pc.connectionState !== 'connected') {
        pc.close();
        this.peers.delete(frame.from);
      }
    }, 25000);
    pc.ondatachannel = ({ channel }) => {
      const peer = new PeerChannel(channel, pc);
      this.peers.set(frame.from, peer);
      let authenticated = false;
      let goodbye: string | undefined;
      let progress: { label: string; done: number; total: number } | undefined;
      peer.onclose = () => {
        clearTimeout(timer);
        this.peers.delete(frame.from);
        if (authenticated)
          this.callbacks.disconnected?.(frame.from, goodbye ?? 'connection lost without goodbye');
        // A controller's activity and progress end with its connection.
        this.callbacks.activity?.(frame.from, '');
        if (progress) this.callbacks.progress?.(frame.from, undefined);
        if (this.device?.abandon(frame.from) && !this.closed)
          this.callbacks.error?.(
            Error('Remote controller disconnected while USB was claimed; completion unknown'),
          );
      };
      void (async () => {
        const hello = decodeMessage(await peer.receive());
        if (
          hello.type !== 'hello' ||
          hello.version !== VERSION ||
          hello.role !== 'controller' ||
          hello.token !== this.ticket.key
        )
          throw Error('Invalid peer authentication');
        authenticated = true;
        clearTimeout(timer);
        this.callbacks.connected?.(frame.from, reason(hello.reason ?? undefined));
        await peer.send(encodeMessage({ type: 'ready', version: VERSION, id: frame.from }));
        const pipeline = new Pipeline((message) =>
          observeRequest(
            frame.from,
            {
              id: message.seq,
              device: message.device,
              operation: message.op,
              timeout_ms: message.timeout_ms,
            },
            () => this.run(frame.from, peer, message),
            this.callbacks.traffic,
          ),
        );
        while (!peer.ended) {
          const message = decodeMessage(await peer.receive());
          if (message.type === 'goodbye') {
            goodbye = reason(message.reason) || 'done';
            peer.close();
            break;
          }
          // Notices arrive in pipeline order, so they describe finished work.
          if (message.type === 'status') {
            this.battery = { level: reason(message.battery), at: Date.now() };
            this.callbacks.battery?.(frame.from, this.battery.level);
            this.callbacks.state?.(this.status());
            continue;
          }
          if (message.type === 'attention') {
            this.callbacks.attention?.(frame.from, reason(message.message));
            continue;
          }
          if (message.type === 'activity') {
            this.callbacks.activity?.(frame.from, reason(message.title));
            continue;
          }
          if (message.type === 'progress_start') {
            progress = { label: reason(message.label), done: 0, total: message.total };
            this.callbacks.progress?.(frame.from, progress);
            continue;
          }
          if (message.type === 'progress_update') {
            if (progress) progress = { ...progress, done: Math.min(message.done, progress.total) };
            this.callbacks.progress?.(frame.from, progress);
            continue;
          }
          if (message.type === 'progress_done') {
            progress = undefined;
            this.callbacks.progress?.(frame.from, undefined);
            continue;
          }
          if (
            !Pipeline.carries(message) ||
            (message.type === 'execute' && (message.timeout_ms < 1 || message.timeout_ms > 120000))
          )
            throw Error('Invalid USB request');
          const reply = await pipeline.handle(message);
          if (reply) await peer.send(encodeMessage(reply));
          // A USB failure ends the connection; nothing queued behind it runs.
          if (reply?.type === 'result' && reply.completion.error) {
            this.callbacks.error?.(Error(reply.completion.error));
            peer.close();
            break;
          }
        }
      })().catch((error) => {
        if (!peer.ended) this.callbacks.error?.(error);
        peer.close();
      });
    };
    await pc.setRemoteDescription(offer);
    await pc.setLocalDescription(await pc.createAnswer());
    await gather(pc);
    this.signal.send({
      type: 'signal',
      to: frame.from,
      data: await seal(this.ticket, frame.from, pc.localDescription!.toJSON()),
    });
  }
  stop() {
    if (this.closed) return;
    this.closed = true;
    this.signal.close();
    this.device?.stopSharing();
    for (const peer of this.peers.values()) peer.close();
    this.peers.clear();
    this.device?.free();
    this.device = undefined;
  }
}
