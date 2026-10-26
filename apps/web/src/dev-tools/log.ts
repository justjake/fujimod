export enum Layer {
  Console = 'Console',
  App80 = 'App80',
  Ff80 = 'FF80',
  Usb = 'USB',
  Connection = 'Connection',
}
export enum Level {
  Error = 'Error',
  Warning = 'Warning',
  Info = 'Info',
  Debug = 'Debug',
  Trace = 'Trace',
}
export enum Direction {
  Host = '·',
  Out = '→',
  In = '←',
}
export interface LogEntry {
  id: number;
  time: number;
  layer: Layer;
  level: Level;
  direction: Direction;
  message: string;
  data?: Uint8Array;
  detail?: string;
  correlation?: string;
}
export type NewEntry = Omit<LogEntry, 'id' | 'time'> & { time?: number };
export function hex(data: Uint8Array): string {
  return Array.from(data, (byte) => byte.toString(16).padStart(2, '0')).join(' ');
}
export function describeCommand(data: Uint8Array, code: number): string {
  if (code === 0x200001 && data.length >= 20) {
    const fn = new DataView(data.buffer, data.byteOffset, data.byteLength).getUint32(16, true);
    return (
      (
        {
          1: 'Read log',
          2: 'Read memory',
          3: 'Write memory',
          5: 'Debug event',
          6: 'Console submit',
          7: 'Console poll',
        } as Record<number, string>
      )[fn] ?? `Debug function ${fn}`
    );
  }
  return (
    (
      {
        0xc0: 'Open session',
        0xc2: 'Ping',
        0xc4: 'Close session',
        0x150000: 'Device info',
        0x1a0002: 'Configuration',
      } as Record<number, string>
    )[code] ?? `Command 0x${code.toString(16)}`
  );
}
export class LogBuffer {
  private nextId = 0;
  private bytes = 0;
  entries: LogEntry[] = [];
  discarded = 0;
  constructor(
    private maxEntries = 2000,
    private maxBytes = 8 * 1024 * 1024,
  ) {}
  private size(entry: LogEntry): number {
    return (entry.message.length + (entry.detail?.length ?? 0)) * 2 + (entry.data?.length ?? 0);
  }
  append(values: NewEntry[]): void {
    for (const value of values) {
      const entry = { ...value, id: this.nextId++, time: value.time ?? Date.now() };
      const bytes = this.size(entry);
      if (bytes > this.maxBytes) {
        this.discarded++;
        continue;
      }
      this.entries.push(entry);
      this.bytes += bytes;
    }
    while (this.entries.length > this.maxEntries || this.bytes > this.maxBytes) {
      this.bytes -= this.size(this.entries.shift()!);
      this.discarded++;
    }
  }
  clear(): void {
    this.entries = [];
    this.bytes = 0;
    this.discarded = 0;
  }
}
/** JSON Lines, one entry per line, without payload bytes; sizes and decoded messages describe each exchange. */
export function exportLog(entries: readonly Omit<LogEntry, 'id'>[]): string {
  return entries
    .map(({ data: _data, ...entry }) =>
      JSON.stringify({ ...entry, time: new Date(entry.time).toISOString() }),
    )
    .map((line) => `${line}\n`)
    .join('');
}
