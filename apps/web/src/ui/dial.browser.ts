/** Exercise dial drag, touch, cancellation, keyboard and feedback in both browsers. */
import { chromium, webkit, type BrowserType, type Locator, type Page } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { parseArgs } from 'node:util';
interface DialSnapshot {
  x: number;
  ticks: number;
  selected: string | null;
  error: number;
}
declare global {
  interface Window {
    dialTouch?: { trusted: boolean; type: string };
  }
}
const fixture =
  '<!doctype html><link rel="stylesheet" href="/src/home/site.css"><div id="controls" style="width:240px;padding:20px"></div><script type="module" src="/src/ui/dial.fixture.tsx"></script>';
async function paint(page: Page) {
  await page.evaluate(
    () =>
      new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))),
  );
}
async function settled(dial: Locator) {
  await dial.evaluate(async (element) => {
    const node = element as HTMLElement;
    await new Promise<void>((done) =>
      requestAnimationFrame(() => requestAnimationFrame(() => done())),
    );
    const values = node.querySelector<HTMLElement>('.vals')!;
    await new Promise<void>((done) => {
      const check = () => {
        const style = getComputedStyle(values);
        const ready = node.classList.contains('ticks')
          ? Math.abs(
              parseFloat(style.backgroundPositionX) - parseFloat(values.style.backgroundPositionX),
            ) < 0.01
          : Math.abs(
              new DOMMatrixReadOnly(style.transform).m41 -
                new DOMMatrixReadOnly(values.style.transform).m41,
            ) < 0.01;
        if (ready) done();
        else requestAnimationFrame(check);
      };
      check();
    });
  });
}
async function snapshot(dial: Locator): Promise<DialSnapshot> {
  return dial.evaluate((element) => {
    const node = element as HTMLElement,
      values = node.querySelector<HTMLElement>('.vals')!,
      on = values.querySelector<HTMLElement>('.on');
    if (!on && !node.classList.contains('ticks')) throw Error('Dial has no selected detent');
    return {
      x: new DOMMatrixReadOnly(getComputedStyle(values).transform).m41,
      ticks: parseFloat(getComputedStyle(values).backgroundPositionX),
      selected: node.getAttribute('aria-valuetext'),
      error: on
        ? on.getBoundingClientRect().x +
          on.offsetWidth / 2 -
          (node.getBoundingClientRect().x + node.clientWidth / 2)
        : 0,
    };
  });
}
async function check(type: BrowserType, url: string, output: string) {
  const browser = await type.launch();
  try {
    const page = await browser.newPage({ viewport: { width: 480, height: 400 }, hasTouch: true }),
      errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.route('**/dial-test.html', (route) =>
      route.fulfill({ body: fixture, contentType: 'text/html' }),
    );
    await page.goto(new URL('/dial-test.html', url).href);
    await page.waitForFunction(() => window.fixtureReady);
    const results: Record<
      string,
      { initial: DialSnapshot; samples: DialSnapshot[]; released: DialSnapshot; timbre: number }
    > = {};
    for (const [id, initialValue, preceding] of [
      ['aperture_ring', '5.6', '4'],
      ['shutter_speed_dial', '125', '60'],
    ] as const) {
      const dial = page.locator(`.dial[data-id="${id}"]`);
      await paint(page);
      const initial = await snapshot(dial),
        box = await dial.boundingBox();
      assert(box);
      const x = box.x + box.width / 2,
        y = box.y + box.height / 2;
      const gap = await dial.evaluate((node) => {
        const labels = [...node.querySelectorAll<HTMLElement>('.vals span')],
          i = Number(node.getAttribute('aria-valuenow'));
        const center = (label: HTMLElement) => label.offsetLeft + label.offsetWidth / 2;
        return center(labels[i]!) - center(labels[i - 1]!);
      });
      const before = await page.evaluate(() => window.requests.length),
        samples: DialSnapshot[] = [],
        travel = Math.round(gap + 4);
      const movements = [0.1, 0.25, 0.4, 0.75]
        .map((fraction) => Math.round(gap * fraction))
        .concat(travel);
      await page.mouse.move(x, y);
      await page.mouse.down();
      for (const dx of movements) {
        await page.mouse.move(x + dx, y);
        await paint(page);
        samples.push(await snapshot(dial));
        assert(
          await page.evaluate(() => window.clicks === window.requests.length),
          'Expected one click per accepted detent',
        );
      }
      for (let i = 0; i < samples.length; i++) {
        assert((i ? samples[i - 1]! : initial).x < samples[i]!.x, JSON.stringify(samples));
        assert(Math.abs(samples[i]!.x - initial.x - movements[i]!) < 0.1);
      }
      assert.deepEqual(
        samples.map((s) => s.selected),
        [initialValue, initialValue, initialValue, preceding, preceding],
      );
      assert.deepEqual(await page.evaluate((start) => window.requests.slice(start), before), [
        { id, value: preceding },
      ]);
      const timbres = await page.evaluate((start) => window.clickTimbres.slice(start), before);
      assert.equal(new Set(timbres).size, 1);
      await page.screenshot({ path: `${output}/${id}-held.png` });
      await page.mouse.up();
      await settled(dial);
      const released = await snapshot(dial);
      assert(Math.abs(released.error) < 1);
      assert(released.x < samples.at(-1)!.x);
      await page.mouse.move(x, y);
      await page.mouse.down();
      await page.mouse.move(x - 8, y);
      await paint(page);
      assert((await snapshot(dial)).x < released.x);
      await dial.dispatchEvent('pointercancel', { pointerId: 1 });
      await page.mouse.up();
      await settled(dial);
      assert(Math.abs((await snapshot(dial)).error) < 1);
      assert.equal(await page.evaluate(() => window.requests.length), before + 1);
      await page.mouse.move(x, y);
      await page.mouse.down();
      await page.mouse.move(x - travel, y);
      await paint(page);
      assert.equal((await snapshot(dial)).selected, initialValue);
      assert(Math.abs((await snapshot(dial)).x - released.x + travel) < 0.1);
      await page.mouse.up();
      await settled(dial);
      assert(Math.abs((await snapshot(dial)).error) < 1);
      await dial.press('ArrowLeft');
      await settled(dial);
      assert.equal((await snapshot(dial)).selected, preceding);
      await dial.press('ArrowRight');
      await settled(dial);
      assert.equal((await snapshot(dial)).selected, initialValue);
      await dial.evaluate((node) =>
        node.addEventListener(
          'pointerdown',
          (event) => {
            if (!(event instanceof PointerEvent)) throw Error('Expected pointer input');
            window.dialTouch = { trusted: event.isTrusted, type: event.pointerType };
            queueMicrotask(() =>
              window.dispatchEvent(
                new PointerEvent('pointermove', {
                  pointerId: event.pointerId,
                  clientX: event.clientX + 8,
                  clientY: event.clientY,
                  buttons: 1,
                }),
              ),
            );
          },
          { once: true },
        ),
      );
      await dial.tap();
      await settled(dial);
      assert.deepEqual(await page.evaluate(() => window.dialTouch), {
        trusted: true,
        type: 'touch',
      });
      assert.equal((await snapshot(dial)).selected, initialValue);
      results[id] = { initial, samples, released, timbre: timbres[0]! };
    }
    assert.notEqual(results.aperture_ring!.timbre, results.shutter_speed_dial!.timbre);
    const ticks = page.locator('.dial[data-id="control_ring"]'),
      box = await ticks.boundingBox();
    assert(box);
    const x = box.x + box.width / 2,
      y = box.y + box.height / 2;
    for (const end of [16, 32]) {
      await page.mouse.move(x, y);
      await page.mouse.down();
      await page.mouse.move(x + 20, y);
      await paint(page);
      assert(Math.abs((await snapshot(ticks)).ticks - end - 4) < 0.01);
      await page.mouse.up();
      await settled(ticks);
      assert(Math.abs((await snapshot(ticks)).ticks - end) < 0.01);
    }
    assert(
      await page.evaluate(() => window.clicks === window.requests.length),
      'Snap or cancellation produced an extra click',
    );
    assert.equal(await page.evaluate(() => new Set(window.clickTimbres).size), 3);
    const aperture = page.locator('.dial[data-id="aperture_ring"]');
    async function touchMove(distance: number) {
      await aperture.evaluate(
        (node, distance) =>
          node.addEventListener(
            'pointerdown',
            (event) => {
              if (!(event instanceof PointerEvent)) throw Error('Expected pointer input');
              queueMicrotask(() =>
                window.dispatchEvent(
                  new PointerEvent('pointermove', {
                    pointerId: event.pointerId,
                    clientX: event.clientX + distance,
                    clientY: event.clientY,
                    buttons: 1,
                  }),
                ),
              );
            },
            { once: true },
          ),
        distance,
      );
      await aperture.tap();
      await settled(aperture);
    }
    await page.evaluate(() => {
      window.haptics = true;
    });
    let before = await page.evaluate(() => ({
      requests: window.requests.length,
      clicks: window.clicks,
      vibrations: window.vibrations.length,
    }));
    const distance = await aperture.evaluate((node) => {
      const labels = [...node.querySelectorAll<HTMLElement>('.vals span')],
        center = (label: HTMLElement) => label.offsetLeft + label.offsetWidth / 2;
      return center(labels[Number(node.getAttribute('aria-valuenow'))]!) - center(labels.at(-1)!);
    });
    const feedback = () =>
      page.evaluate(
        (before) => ({
          steps: window.requests.length - before.requests,
          clicks: window.clicks - before.clicks,
          vibrations: window.vibrations.slice(before.vibrations),
        }),
        before,
      );
    await touchMove(distance);
    assert.deepEqual(await feedback(), { steps: 3, clicks: 0, vibrations: [0, 8, 8, 8] });
    before = await page.evaluate(() => ({
      requests: window.requests.length,
      clicks: window.clicks,
      vibrations: window.vibrations.length,
    }));
    await touchMove(-40);
    assert.deepEqual(await feedback(), { steps: 0, clicks: 0, vibrations: [0] });
    await page.evaluate(() => {
      window.haptics = false;
    });
    const prior = await page.evaluate(() => ({
      requests: window.requests.length,
      clicks: window.clicks,
    }));
    await touchMove(-distance);
    assert.deepEqual(
      await page.evaluate(
        (before) => ({
          steps: window.requests.length - before.requests,
          clicks: window.clicks - before.clicks,
        }),
        prior,
      ),
      { steps: 3, clicks: 3 },
    );
    await page.screenshot({ path: `${output}/released.png` });
    assert.deepEqual(errors, []);
    await writeFile(
      `${output}/result.json`,
      JSON.stringify(
        {
          dials: results,
          requests: await page.evaluate(() => window.requests),
          feedback: await page.evaluate(() => ({
            clicks: window.clicks,
            vibrations: window.vibrations,
            clickTimbres: window.clickTimbres,
          })),
        },
        null,
        2,
      ) + '\n',
    );
  } finally {
    await browser.close();
  }
}
const { values } = parseArgs({ options: { url: { type: 'string' }, output: { type: 'string' } } });
assert(values.url && values.output, 'Supply --url and --output inside the web build directory.');
const output = resolve(values.output),
  build = resolve('build');
assert(output.startsWith(build + '/'), 'Dial proof output must remain in the web build directory.');
for (const type of [chromium, webkit]) {
  const target = `${output}/${type.name()}`;
  await mkdir(target, { recursive: true });
  await check(type, values.url, target);
  console.log(
    `${type.name()}: dial drag, snap, reversal, cancellation, keyboard, touch and feedback passed.`,
  );
}
