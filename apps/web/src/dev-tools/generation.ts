import { catalog } from 'fujimod';

/** Explain an absent artifact using the outcome its generator published. */
export function generationFailure(firmwareId: string, table: string): string {
  const outcome = catalog.generationResults().find((entry) => entry.firmware_id === firmwareId)
    ?.tables[table];
  if (outcome?.errors.length)
    return outcome.errors.map((error) => `${error.locator}: ${error.reason}`).join('; ');
  return outcome
    ? `The generated ${table} artifact is missing for ${firmwareId}.`
    : `No ${table} generation result was published for ${firmwareId}.`;
}
