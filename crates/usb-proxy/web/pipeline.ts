import type { Message, ExecuteMessage, Completion } from './types.ts';
import { expectationHolds } from './wire.ts';
// IN results kept since the last sync for expectations, oldest dropped first.
const KEPT_BYTES = 16 * 1024 * 1024;
const empty = new Uint8Array();
/**
 * One controller's pipeline at an exporter (PIPELINE.md). Operations run
 * strictly in order, one at a time. After a failed operation or expectation,
 * later operations and expectations are discarded without touching USB until
 * the next sync, which reports how many were discarded.
 */
export class Pipeline {
  execute: (message: ExecuteMessage) => Promise<Completion>;
  failed = false;
  discarded = 0;
  results = new Map<number, Uint8Array>();
  kept = 0;
  constructor(execute: (message: ExecuteMessage) => Promise<Completion>) {
    this.execute = execute;
  }
  /** Whether `message` belongs to the pipeline rather than being a notice. */
  static carries(message: Message) {
    return message.type === 'execute' || message.type === 'expect' || message.type === 'sync';
  }
  /** Run one pipeline message; returns the reply to send, if any. */
  async handle(message: Message): Promise<Message | undefined> {
    if (message.type === 'sync') {
      const reply: Message = { type: 'synced', seq: message.seq, discarded: this.discarded };
      this.failed = false;
      this.discarded = 0;
      this.results.clear();
      this.kept = 0;
      return reply;
    }
    if (message.type !== 'execute' && message.type !== 'expect')
      throw Error('Invalid pipeline message');
    if (this.failed) {
      this.discarded++;
      return;
    }
    if (message.type === 'expect') {
      const data = this.results.get(message.of);
      if (
        (message.length == null || data?.length === message.length) &&
        (expectationHolds(data, message.offset, message.bytes, message.mask) ||
          message.alternatives?.some((bytes) =>
            expectationHolds(data, message.offset, bytes, message.mask),
          ))
      )
        return;
      this.failed = true;
      return {
        type: 'expect_failed',
        seq: message.seq,
        actual: data?.slice(message.offset, message.offset + message.bytes.length) ?? empty,
      };
    }
    let completion = await this.execute(message);
    if (
      !completion.error &&
      (message.op.op === 'control_out' || message.op.op === 'transfer_out') &&
      (completion.transferred ?? 0) !== (message.op.data?.length ?? 0)
    )
      completion = { ...completion, error: 'short OUT transfer' };
    if (completion.error) this.failed = true;
    else if (completion.data?.length) this.keep(message.seq, completion.data);
    return { type: 'result', seq: message.seq, completion };
  }
  keep(seq: number, data: Uint8Array) {
    this.results.set(seq, data);
    this.kept += data.length;
    for (const [oldest, bytes] of this.results) {
      if (this.kept <= KEPT_BYTES) break;
      this.results.delete(oldest);
      this.kept -= bytes.length;
    }
  }
}
