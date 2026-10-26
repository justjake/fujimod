/** Cumulative host costs; snapshots allow phase-local differences without resetting execution. */
export class HostTimings {
  private rows: Record<string, { calls: number; milliseconds: number; maximum: number }> = {};
  measure<T>(name: string, operation: () => T): T {
    const start = performance.now();
    try {
      return operation();
    } finally {
      const elapsed = performance.now() - start;
      const row = (this.rows[name] ??= { calls: 0, milliseconds: 0, maximum: 0 });
      row.calls++;
      row.milliseconds += elapsed;
      row.maximum = Math.max(row.maximum, elapsed);
    }
  }
  async measureAsync<T>(name: string, operation: () => Promise<T>): Promise<T> {
    const start = performance.now();
    try {
      return await operation();
    } finally {
      const elapsed = performance.now() - start;
      const row = (this.rows[name] ??= { calls: 0, milliseconds: 0, maximum: 0 });
      row.calls++;
      row.milliseconds += elapsed;
      row.maximum = Math.max(row.maximum, elapsed);
    }
  }
  snapshot() {
    return structuredClone(this.rows);
  }
}
