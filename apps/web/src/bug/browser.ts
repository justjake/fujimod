declare global {
  interface Window {
    __reportUpload: {
      digest: string;
      description: FormDataEntryValue | null;
      metadata: Record<string, unknown>;
    };
  }
}
// Exercise the page with the real TAR produced by a passed camera walkthrough.
import { chromium, webkit } from 'playwright';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { gunzipSync } from 'node:zlib';
import assert from 'node:assert/strict';
const directory = process.argv[2],
  base = process.env.FUJIMOD_WEB_URL;
if (!directory || !base)
  throw Error('Set FUJIMOD_WEB_URL and pass the completed camera guide directory.');
const output = fileURLToPath(new URL('../../build/bug-report/', import.meta.url));
await mkdir(output, { recursive: true });
const guide = JSON.parse(await readFile(`${directory}/guide.json`, 'utf8'));
assert.equal(guide.passed, true);
assert.equal(guide.identity.walkthrough, 'bug-report/report/capture');
const url = new URL(guide.bug_report.issue_url);
const destination = new URL(base);
url.protocol = destination.protocol;
url.host = destination.host;
await writeFile(`${output}/preview-url.txt`, url.href);
const engine = process.env.FUJIMOD_BROWSER === 'webkit' ? webkit : chromium;
const browser = await engine.launch({ headless: true });
try {
  const context = await browser.newContext({
    acceptDownloads: true,
    ...(engine === webkit
      ? {
          userAgent:
            'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1',
        }
      : {}),
    viewport: { width: 390, height: 844 },
  });
  const sent: string[] = [],
    issues: string[] = [],
    errors: string[] = [];
  context.on('request', (request) => {
    if (!['GET', 'HEAD'].includes(request.method())) sent.push(request.url());
  });
  await context.route('https://share.fujimod.com/issue-reports', async (route) => {
    const request = route.request();
    console.log(engine.name(), 'intercepted explicit upload');
    if (engine === chromium) {
      const form = await new Request(request.url(), {
        method: 'POST',
        headers: request.headers(),
        body: new Uint8Array(request.postDataBuffer()!),
      }).formData();
      assert.deepEqual(
        Buffer.from(await (form.get('report') as File).arrayBuffer()),
        await readFile(`${directory}/report.tar`),
      );
      assert.equal(form.get('description'), 'The menu stopped responding after a mod callback.');
      const metadata = JSON.parse(String(form.get('metadata')));
      assert.equal(metadata.bytes, guide.bug_report.bytes);
      assert.equal(metadata.archive, undefined);
    }
    issues.push(request.url());
    return route.fulfill({
      status: 201,
      contentType: 'application/json',
      body: JSON.stringify({
        id: request.headers()['idempotency-key'],
        issueURL: 'https://github.com/justjake/fujimod/issues/123',
      }),
    });
  });
  const page = await context.newPage();
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(() => {
    const send = window.fetch;
    window.fetch = async (...args) => {
      const body = args[1]?.body;
      if (body instanceof FormData && body.has('report')) {
        const file = body.get('report') as File;
        const digest = await crypto.subtle.digest('SHA-256', await file.arrayBuffer());
        window.__reportUpload = {
          digest: [...new Uint8Array(digest)].map((n) => n.toString(16).padStart(2, '0')).join(''),
          description: body.get('description'),
          metadata: JSON.parse(String(body.get('metadata'))),
        };
      }
      return send(...args);
    };
  });
  await page.goto(url.href);
  console.log(engine.name(), 'loaded report page');
  const usbAvailable = await page.evaluate(() => isSecureContext && 'usb' in navigator);
  const usbCard = page.getByRole('region', { name: 'USB connection' });
  await usbCard
    .getByRole('heading', {
      name: usbAvailable ? 'Connect over USB' : 'WebUSB unavailable',
      exact: true,
    })
    .waitFor();
  assert.equal(
    await usbCard.evaluate((e) => e.classList.contains('bug-usb-unavailable')),
    !usbAvailable,
  );
  assert.equal(await page.locator('h1').count(), 0);
  await page
    .getByText(
      'Your camera data is private and remains on your device until you press upload. Uploaded issue reports contain this data:',
      { exact: true },
    )
    .waitFor();
  await page.locator('.bug-platform').waitFor();
  if (engine === webkit)
    assert.match((await page.locator('.bug-platform').textContent())!, /iPhone · Files/);
  await page.screenshot({ path: `${output}/browser-phone.png`, fullPage: true });
  assert.equal(await page.locator('body').evaluate((e) => e.scrollWidth > innerWidth), false);
  await page.locator('input[type=file]').setInputFiles(`${directory}/report.tar`);
  await page.getByText('Report ready', { exact: true }).waitFor();
  console.log(engine.name(), 'opened and validated TAR');
  assert.deepEqual(sent, []);
  const button = page.getByRole('button', { name: 'Download all', exact: true });
  await page.waitForFunction(
    () =>
      ![...document.querySelectorAll<HTMLButtonElement>('.bug-actions button')].some(
        (node) => node.disabled,
      ),
  );
  const downloadPromise = page.waitForEvent('download');
  await button.click();
  const download = await downloadPromise;
  console.log(engine.name(), 'downloaded local copy');
  await download.saveAs(`${output}/${download.suggestedFilename()}`);
  assert.deepEqual(
    gunzipSync(await readFile(`${output}/${download.suggestedFilename()}`)),
    await readFile(`${directory}/report.tar`),
  );
  assert.deepEqual(sent, []);
  await page.getByRole('button', { name: 'Contribute', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Contribute' });
  await dialog.getByText('Your message will be posted publicly on GitHub.').waitFor();
  await dialog.getByRole('textbox').fill('The menu stopped responding after a mod callback.');
  assert.deepEqual(sent, []);
  await dialog.getByRole('button', { name: 'Upload', exact: true }).click();
  await dialog.getByText('Uploaded', { exact: false }).waitFor();
  const captured = await page.evaluate(() => window.__reportUpload);
  assert.equal(
    captured.digest,
    createHash('sha256')
      .update(await readFile(`${directory}/report.tar`))
      .digest('hex'),
  );
  assert.equal(captured.description, 'The menu stopped responding after a mod callback.');
  assert.equal(captured.metadata.archive, undefined);
  assert.equal(issues.length, 1);
  assert.deepEqual(sent, ['https://share.fujimod.com/issue-reports']);
  assert.deepEqual(errors, []);
  await page.screenshot({ path: `${output}/browser-ready-phone.png`, fullPage: true });
  await page.setViewportSize({ width: 1280, height: 960 });
  await page.screenshot({ path: `${output}/browser-ready-desktop.png`, fullPage: true });
  await writeFile(
    `${output}/browser-check.json`,
    JSON.stringify(
      {
        passed: true,
        url: url.href,
        archiveBytes: guide.bug_report.bytes,
        download: download.suggestedFilename(),
        outgoingWrites: sent,
        pageErrors: errors,
      },
      null,
      2,
    ),
  );
  console.log(
    'PASS',
    engine.name(),
    'real camera walkthrough TAR, responsive page, explicit upload only, exact TAR transfer and local download.',
  );
} finally {
  await browser.close();
}
