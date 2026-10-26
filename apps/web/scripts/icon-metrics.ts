// Measures every symbol in the feature icon sprite and writes the sizes that give
// all icons the same optical size (src/ui/assets/feature-icon-metrics.generated.json).
//
//   node scripts/icon-metrics.ts           # regenerate
//   node scripts/icon-metrics.ts --check   # fail if the checked-in file is stale
//
// Each icon is cropped to its ink (stroke included) and scaled so that
// √(width × height) of the ink is one unit; icons with more ink than a typical
// outline icon shrink by up to 8%. Heights and widths are capped so very tall or
// wide shapes do not sprawl. Sizes are in multiples of the target the page sets
// (a multiple of the capital height of the adjacent text).
import { readFile, writeFile } from 'node:fs/promises';
import { chromium } from 'playwright';

const sprite = new URL('../src/ui/assets/feature-icons.svg', import.meta.url);
const output = new URL('../src/ui/assets/feature-icon-metrics.generated.json', import.meta.url);

/** Ink coverage of a typical outline icon; fuller icons are optically heavier. */
const OUTLINE_COVERAGE = 0.35;
const SOLID_CORRECTION = 0.16;
const MIN_SOLID_SCALE = 0.92;
/** Caps relative to the target size. */
const MAX_HEIGHT = 1.185;
const MAX_WIDTH = 1.556;

type Box = [number, number, number, number];
type Measured = { box: Box; view: Box; coverage: number };
/** Ink box and the symbol's own viewBox (symbol units), and the optical width and height. */
export type IconMetric = { box: Box; view: Box; width: number; height: number };

async function measure(svg: string): Promise<Record<string, Measured>> {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.setContent(
      `<!doctype html><body>${svg.replace('<svg ', '<svg style="display:none" ')}</body>`,
    );
    return await page.evaluate(async () => {
      const out: Record<string, Measured> = {};
      for (const symbol of document.querySelectorAll('symbol')) {
        const viewBox = symbol.getAttribute('viewBox')!;
        const probe = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        probe.setAttribute('viewBox', viewBox);
        probe.style.cssText = 'position:absolute;width:100px;height:100px';
        probe.innerHTML = symbol.innerHTML;
        document.body.append(probe);
        const b = probe.getBBox();
        const strokes = [...probe.querySelectorAll('[stroke-width]')]
          .filter((e) => e.getAttribute('stroke') !== 'none')
          .map((e) => parseFloat(e.getAttribute('stroke-width')!) / 2);
        probe.remove();
        const half = Math.max(0, ...strokes);
        const box: Box = [b.x - half, b.y - half, b.width + 2 * half, b.height + 2 * half];
        const view = viewBox.split(/[\s,]+/).map(Number) as Box;
        // Coverage: the share of the ink box that is ink, rasterized at 256 px.
        const scale = 256 / Math.max(box[2], box[3]);
        const width = Math.round(box[2] * scale);
        const height = Math.round(box[3] * scale);
        const xml =
          `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${box.join(' ')}" width="${width}" height="${height}" color="#fff">` +
          symbol.innerHTML.replaceAll('currentColor', '#fff') +
          '</svg>';
        const image = new Image();
        image.src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(xml);
        await image.decode();
        const canvas = document.createElement('canvas');
        canvas.width = width;
        canvas.height = height;
        const context = canvas.getContext('2d')!;
        context.drawImage(image, 0, 0);
        const pixels = context.getImageData(0, 0, width, height).data;
        let ink = 0;
        for (let i = 3; i < pixels.length; i += 4) ink += pixels[i] / 255;
        out[symbol.id] = { box, view, coverage: ink / (width * height) };
      }
      return out;
    });
  } finally {
    await browser.close();
  }
}

const round = (value: number) => Math.round(value * 1000) / 1000;

function size({ box, view, coverage }: Measured): IconMetric {
  const [, , w, h] = box;
  let s = 1 / Math.sqrt(w * h);
  s *= Math.max(MIN_SOLID_SCALE, Math.min(1, 1 - SOLID_CORRECTION * (coverage - OUTLINE_COVERAGE)));
  s = Math.min(s, MAX_HEIGHT / h, MAX_WIDTH / w);
  return { box: box.map(round) as Box, view, width: round(w * s), height: round(h * s) };
}

const measured = await measure(await readFile(sprite, 'utf8'));
const metrics = Object.fromEntries(Object.entries(measured).map(([id, m]) => [id, size(m)]));
// The repository formatter's layout: one object per icon, number arrays on one line.
const text =
  '{\n' +
  Object.entries(metrics)
    .map(
      ([id, m]) =>
        `  ${JSON.stringify(id)}: {\n` +
        `    "box": [${m.box.join(', ')}],\n    "view": [${m.view.join(', ')}],\n` +
        `    "width": ${m.width},\n    "height": ${m.height}\n  }`,
    )
    .join(',\n') +
  '\n}\n';
if (process.argv.includes('--check')) {
  const current = await readFile(output, 'utf8').catch(() => '');
  if (current !== text) {
    console.error(`${output.pathname} is stale; run node scripts/icon-metrics.ts`);
    process.exit(1);
  }
} else {
  await writeFile(output, text);
  console.log(`${Object.keys(metrics).length} icons measured`);
}
