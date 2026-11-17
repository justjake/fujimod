/**
 * The display a camera with several scanout outputs is using.
 *
 * A body with a rear LCD and an electronic viewfinder drives each panel from
 * its own scanout output, and its firmware stops presenting on the panel it
 * switches off. The output that presented most recently is therefore the one
 * in use. Before any output presents, the first listed output is shown.
 */
export class PresentedDisplay {
  readonly #sequences = new Map<number, bigint>();
  #current: number;
  #presentations = 0;

  constructor(readonly outputs: readonly number[]) {
    if (outputs.length === 0) throw new Error('A presented display needs at least one output.');
    this.#current = outputs[0]!;
  }

  /**
   * Record each output's latest frame sequence and return the output to show,
   * with a sequence that advances whenever any listed output presents. When
   * several outputs presented since the last call, the current one stays if it
   * is among them; otherwise the first of them in list order is shown.
   */
  observe(sequence: (output: number) => bigint | undefined): { output: number; sequence: number } {
    const presented: number[] = [];
    for (const output of this.outputs) {
      const value = sequence(output);
      if (value === undefined || this.#sequences.get(output) === value) continue;
      this.#sequences.set(output, value);
      presented.push(output);
    }
    if (presented.length > 0) {
      this.#presentations += 1;
      if (!presented.includes(this.#current)) this.#current = presented[0]!;
    }
    return { output: this.#current, sequence: this.#presentations };
  }
}
