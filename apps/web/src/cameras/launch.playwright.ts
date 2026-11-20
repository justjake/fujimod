import { readFile } from 'node:fs/promises';
import type { BrowserContext, Page } from 'playwright';

/** Serve the firmware mirror download from a local copy of the X70 1.10 update. */
export async function serveFirmware(target: Page | BrowserContext, dat: string) {
  const bytes = await readFile(dat);
  await target.route('**/proxy?*', (route) =>
    route.fulfill({ body: bytes, contentType: 'application/octet-stream' }),
  );
}

/** Press Launch Fujimod, as a visitor does, with the firmware served from `dat`. */
export async function launchFujimod(page: Page, dat?: string) {
  if (dat) await serveFirmware(page, dat);
  await page.locator('.launch-fujimod:not([disabled])').click();
}
