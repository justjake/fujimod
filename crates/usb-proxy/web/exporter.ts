import { VERSION, encodeMessage, decodeMessage } from './wire.ts';
import { Pipeline } from './pipeline.ts';
import type { Message, Descriptor, UsbExporter } from './types.ts';

interface ExporterSdk {
  initialize(): Promise<unknown>;
  usb: {
    Exporter: {
      discover(): Promise<UsbExporter>;
      requestDevice(): Promise<UsbExporter | undefined>;
    };
  };
}
// The Rust relay serves the built browser binding beside this transport page.
const sdk = () => {
  const url = new URL('/usb-proxy/bindings/dist/index.js', location.href).href;
  return import(url) as Promise<ExporterSdk>;
};
if (typeof document !== 'undefined') {
  const $ = (id: string) => {
    const element = document.getElementById(id);
    if (!element) throw Error(`Missing element ${id}`);
    return element;
  };
  const button = (id: string) => {
    const element = $(id);
    if (!(element instanceof HTMLButtonElement)) throw Error(`Expected button ${id}`);
    return element;
  };
  let socket: WebSocket | undefined,
    exporter: UsbExporter | undefined,
    binding: ExporterSdk | undefined;
  const controllers = new Set<string>();
  const status = (text: string) => {
    $('status').textContent = text;
  };
  const devices = (): Descriptor[] =>
    exporter ? (JSON.parse(exporter.inventoryJson) as Descriptor[]) : [];
  const update = () => {
    $('devices').replaceChildren(
      ...devices().map((device) => {
        const row = document.createElement('tr');
        for (const value of [
          device.product || device.manufacturer || 'USB device',
          `${device.vendor_id.toString(16).padStart(4, '0')}:${device.product_id.toString(16).padStart(4, '0')}`,
          exporter!.fenced(device.id)
            ? 'Completion unknown'
            : exporter!.busy(device.id)
              ? 'In use'
              : 'Available',
        ]) {
          const cell = document.createElement('td');
          cell.textContent = value;
          row.append(cell);
        }
        return row;
      }),
    );
  };
  const inventory = () => {
    if (exporter && socket?.readyState === WebSocket.OPEN)
      socket.send(encodeMessage({ type: 'inventory', devices: devices() }));
    update();
  };
  const disconnected = async () => {
    const stopping = exporter,
      active = socket;
    socket = undefined;
    exporter = undefined;
    active?.close();
    for (const id of ['add', 'refresh', 'disconnect', 'connect']) button(id).disabled = true;
    status('Disconnecting');
    try {
      for (const controller of controllers) stopping?.abandon(controller);
      controllers.clear();
      await stopping?.shutdown();
    } finally {
      stopping?.free();
      button('connect').disabled = false;
      status('Disconnected');
      update();
    }
  };
  $('connect-form').onsubmit = async (event) => {
    event.preventDefault();
    if (!navigator.usb) {
      status('WebUSB unavailable');
      return;
    }
    button('connect').disabled = true;
    status('Connecting');
    try {
      binding = await sdk();
      await binding.initialize();
      exporter = await binding.usb.Exporter.discover();
    } catch (error) {
      status(String(error));
      button('connect').disabled = false;
      return;
    }
    socket = new WebSocket(
      `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/usb-proxy/ws`,
    );
    socket.binaryType = 'arraybuffer';
    const active = socket;
    socket.onopen = () =>
      active.send(
        encodeMessage({
          type: 'hello',
          version: VERSION,
          role: 'exporter',
          token: ($('token') as HTMLInputElement).value,
          reason: null,
        }),
      );
    const streams = new Map<number, { pipeline: Pipeline; chain: Promise<void> }>();
    socket.onmessage = ({ data }) => {
      const message = decodeMessage(data);
      if (message.type === 'ready' && message.version === VERSION) {
        status(`Connected · ${message.id}`);
        for (const id of ['add', 'refresh', 'disconnect']) button(id).disabled = false;
        inventory();
        return;
      }
      if (message.type !== 'routed') return;
      const { stream, message: inner } = message;
      const controller = String(stream);
      if (inner.type === 'goodbye') {
        exporter?.abandon(controller);
        controllers.delete(controller);
        streams.delete(stream);
        update();
        return;
      }
      if (!Pipeline.carries(inner) || !exporter) return;
      controllers.add(controller);
      let entry = streams.get(stream);
      if (!entry) {
        entry = {
          pipeline: new Pipeline(async (request) => {
            if (!exporter) throw Error('USB exporter disconnected');
            const reply = decodeMessage(await exporter.execute(controller, encodeMessage(request)));
            if (reply.type !== 'result' || reply.seq !== request.seq)
              throw Error('Invalid Rust exporter reply');
            return reply.completion;
          }),
          chain: Promise.resolve(),
        };
        streams.set(stream, entry);
      }
      const { pipeline } = entry;
      entry.chain = entry.chain
        .then(async () => {
          const answer = await pipeline.handle(inner);
          if (answer && active.readyState === WebSocket.OPEN)
            active.send(encodeMessage({ type: 'routed', stream, message: answer }));
          update();
        })
        .catch((error: unknown) => status(String(error)));
    };
    socket.onclose = () => {
      if (socket === active) void disconnected();
    };
    socket.onerror = () => status('Connection failed');
  };
  $('add').onclick = async () => {
    try {
      const next = await binding?.usb.Exporter.requestDevice();
      if (next) {
        const old = exporter;
        exporter = next;
        old?.free();
        inventory();
      }
    } catch (error) {
      status(String(error));
    }
  };
  $('refresh').onclick = async () => {
    try {
      const next = await binding?.usb.Exporter.discover();
      if (next) {
        const old = exporter;
        exporter = next;
        old?.free();
        inventory();
      }
    } catch (error) {
      status(String(error));
    }
  };
  $('disconnect').onclick = () => {
    void disconnected();
  };
  navigator.usb?.addEventListener('connect', inventory);
  navigator.usb?.addEventListener('disconnect', inventory);
}
