import PHOTO from '../../../engine/assets/default-scene.png';
import { Show, createEffect, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

import {
  CLARITY_STEPS,
  H,
  LEVELS,
  NEUTRAL_KNOTS,
  RAWS,
  W,
  ZERO_TILT,
  axis,
  bandGains,
  chromaMatrix,
  curveEval,
  decode,
  handlesOf,
  mixMatrix,
  type Film,
  type RecipeValues,
} from './model';

export interface Source {
  pixels: Uint8ClampedArray;
  /** Input brightness on the stops axis, 64 bins. */
  histogram: number[];
}

export const DECODE = Array.from({ length: 256 }, (_, v) => decode(v / 255));

const sources = new Map<number, Promise<Source>>();

/** Crop 0 is live view; crop n is RAWS[n - 1]. */
export function source(crop = 0): Promise<Source> {
  const cached = sources.get(crop);
  if (cached) return cached;
  const promise = new Promise<Source>((resolve) => {
    const image = new Image();
    image.onload = () => {
      const canvas = document.createElement('canvas');
      canvas.width = W;
      canvas.height = H;
      const context = canvas.getContext('2d');
      if (!context) throw new Error('No 2D context');
      const raw = RAWS[crop - 1];
      const scale = Math.max(W / image.width, H / image.height) * (raw?.zoom ?? 1);
      const w = image.width * scale;
      const h = image.height * scale;
      const x = raw ? Math.min(0, Math.max(W - w, W / 2 - raw.x * w)) : (W - w) / 2;
      const y = raw ? Math.min(0, Math.max(H - h, H / 2 - raw.y * h)) : (H - h) / 2;
      context.drawImage(image, x, y, w, h);
      const pixels = context.getImageData(0, 0, W, H).data;
      const histogram = Array.from({ length: 64 }, () => 0);
      for (let p = 0; p < pixels.length; p += 4) {
        const l =
          0.2126 * DECODE[pixels[p]] +
          0.7152 * DECODE[pixels[p + 1]] +
          0.0722 * DECODE[pixels[p + 2]];
        histogram[Math.min(63, Math.floor(axis(l * 4096) * 64))]++;
      }
      const peak = Math.max(1, ...histogram);
      resolve({ pixels, histogram: histogram.map((n) => Math.sqrt(n / peak)) });
    };
    image.src = PHOTO;
  });
  sources.set(crop, promise);
  return promise;
}

const NEUTRAL_LUT = (() => {
  const hs = handlesOf(NEUTRAL_KNOTS, ZERO_TILT);
  const lut = new Float32Array(4097);
  for (let x = 0; x <= 4096; x++) lut[x] = curveEval(NEUTRAL_KNOTS, hs, axis(x));
  return lut;
})();

function buildChannelLuts(f: Film): [Float32Array, Float32Array, Float32Array] {
  const hsAll = handlesOf(f.knots, f.tilt);
  const hsR = handlesOf(f.channelKnots.r, f.channelTilt.r);
  const hsG = handlesOf(f.channelKnots.g, f.channelTilt.g);
  const hsB = handlesOf(f.channelKnots.b, f.channelTilt.b);
  const lutR = new Float32Array(4097);
  const lutG = new Float32Array(4097);
  const lutB = new Float32Array(4097);
  for (let x = 0; x <= 4096; x++) {
    const u = axis(x);
    const base = curveEval(f.knots, hsAll, u);
    const neutral = NEUTRAL_LUT[x];
    lutR[x] = Math.min(1, Math.max(0, base + (curveEval(f.channelKnots.r, hsR, u) - neutral)));
    lutG[x] = Math.min(1, Math.max(0, base + (curveEval(f.channelKnots.g, hsG, u) - neutral)));
    lutB[x] = Math.min(1, Math.max(0, base + (curveEval(f.channelKnots.b, hsB, u) - neutral)));
  }
  return [lutR, lutG, lutB];
}

function numericLevel(recipe: RecipeValues, id: string): number {
  const v = LEVELS[recipe[id] ?? 2];
  return typeof v === 'number' ? v : 0;
}

function clarityLevel(recipe: RecipeValues): number {
  const v = CLARITY_STEPS[recipe.clarity ?? 5];
  return typeof v === 'number' ? v : 0;
}

function develop(f: Film, recipe: RecipeValues, filmOnly: boolean, src: Source, out: ImageData) {
  const [lutR, lutG, lutB] = buildChannelLuts(f);
  const m = mixMatrix(f);
  const [a00, a01, a10, a11] = chromaMatrix(f);
  const band = bandGains(f);

  /* Recipe adjustments (neutralized when FILM ONLY is active). */
  const wbPresetWarm = [0, 0, 0.03, 0, 0.06, -0.05, -0.04, -0.03, -0.1, 0.02][recipe.wb ?? 0] ?? 0;
  const wbR = filmOnly ? 1 : 1 + recipe.shiftR * 0.018 + wbPresetWarm;
  const wbB = filmOnly ? 1 : 1 + recipe.shiftB * 0.018 - wbPresetWarm;
  const wbG = filmOnly ? 1 : 1 - recipe.shiftR * 0.005 - recipe.shiftB * 0.004;

  const hiTone = filmOnly ? 0 : numericLevel(recipe, 'highlight') * 0.045;
  const shTone = filmOnly ? 0 : numericLevel(recipe, 'shadow') * 0.045;
  const colorMul = filmOnly ? 1 : 1 + numericLevel(recipe, 'color') * 0.14;
  const chrome = filmOnly ? 0 : (recipe.chrome ?? 0) * 0.07;
  const fxBlue = filmOnly ? 0 : (recipe.fxblue ?? 0) * 0.09;
  const clarity = filmOnly ? 0 : clarityLevel(recipe) * 0.022;
  const grainAmp = filmOnly ? 0 : (recipe.grain ?? 0) * 0.032;
  const grainLarge = (recipe.grainSize ?? 0) === 1;
  const halation = filmOnly ? 0 : (recipe.halation ?? 0) * 0.08;
  const mist = filmOnly ? 0 : (recipe.mist ?? 0) * 0.045;
  const leaks = filmOnly ? 0 : (recipe.leaks ?? 0) * 0.12;

  const px = src.pixels;
  const o = out.data;

  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      const p = (y * W + x) * 4;
      const r = DECODE[px[p]] * wbR;
      const g = DECODE[px[p + 1]] * wbG;
      const b = DECODE[px[p + 2]] * wbB;

      const R =
        lutR[Math.min(4096, Math.max(0, Math.round((m[0] * r + m[1] * g + m[2] * b) * 4096)))];
      const G =
        lutG[Math.min(4096, Math.max(0, Math.round((m[3] * r + m[4] * g + m[5] * b) * 4096)))];
      const B =
        lutB[Math.min(4096, Math.max(0, Math.round((m[6] * r + m[7] * g + m[8] * b) * 4096)))];

      let Y = 0.299 * R + 0.587 * G + 0.114 * B;
      const cb = (B - Y) * 0.564;
      const cr = (R - Y) * 0.713;

      const gain = band[Math.min(255, Math.max(0, Math.round(Y * 255)))] * colorMul;
      const cb2 = (a00 * cb + a01 * cr) * gain;
      const cr2 = (a10 * cb + a11 * cr) * gain;

      if (!filmOnly) {
        /* Highlight & shadow tone shaping + clarity midtone punch. */
        if (Y > 0.5) Y += hiTone * (Y - 0.5) * 2;
        else Y -= shTone * (0.5 - Y) * 2;
        if (clarity !== 0) Y += clarity * Math.sin((Y - 0.5) * Math.PI);
        if (chrome > 0) {
          const chromaMag = Math.hypot(cb2, cr2);
          Y -= chrome * chromaMag * 0.7;
        }
        if (fxBlue > 0 && cb2 > 0.05) {
          Y -= fxBlue * cb2 * 0.85;
        }
        if (mist > 0) {
          Y = Y * (1 - mist * 0.45) + mist * 0.42;
        }
      }

      let R2 = Y + 1.402 * cr2;
      let B2 = Y + 1.773 * cb2;
      let G2 = (Y - 0.299 * R2 - 0.114 * B2) / 0.587;

      if (!filmOnly) {
        if (halation > 0 && Y > 0.72) {
          const glow = (Y - 0.72) * halation * 2.2;
          R2 += glow;
          G2 += glow * 0.35;
        }
        if (leaks > 0) {
          const edge = Math.max(0, (x / W - 0.62) / 0.38);
          const fall = edge * edge * (0.65 + 0.35 * Math.sin(y * 0.015));
          R2 += leaks * fall * 1.4;
          G2 += leaks * fall * 0.52;
          B2 += leaks * fall * 0.1;
        }
        if (grainAmp > 0) {
          const gx = grainLarge ? x >> 1 : x;
          const gy = grainLarge ? y >> 1 : y;
          const h = ((gx * 374761393 + gy * 668265263) ^ (gx * 1274126177)) & 0xff;
          const n = (h / 255 - 0.5) * 2 * grainAmp;
          R2 += n;
          G2 += n;
          B2 += n;
        }
      }

      o[p] = Math.min(255, Math.max(0, R2 * 255));
      o[p + 1] = Math.min(255, Math.max(0, G2 * 255));
      o[p + 2] = Math.min(255, Math.max(0, B2 * 255));
      o[p + 3] = 255;
    }
  }
}

function drawOverlays(context: CanvasRenderingContext2D, recipe: RecipeValues, filmOnly: boolean) {
  if (filmOnly) return;
  const borders = recipe.borders ?? 0;
  if (borders > 0) {
    const pad = borders === 1 ? 10 : 20;
    context.strokeStyle = '#08080a';
    context.lineWidth = pad * 2;
    context.strokeRect(0, 0, W, H);
    if (borders === 2) {
      context.strokeStyle = 'rgba(255,255,255,0.22)';
      context.lineWidth = 1;
      context.strokeRect(pad + 0.5, pad + 0.5, W - pad * 2 - 1, H - pad * 2 - 1);
    }
  }
  if ((recipe.dateStamp ?? 0) === 1) {
    context.save();
    context.font = '700 22px monospace';
    context.fillStyle = 'rgba(255, 138, 36, 0.92)';
    context.shadowColor = 'rgba(255, 80, 10, 0.75)';
    context.shadowBlur = 6;
    context.textAlign = 'right';
    context.fillText("'26 11 26", W - 24, H - 24);
    context.restore();
  }
}

function Canvas(props: {
  film: Film;
  recipe: RecipeValues;
  filmOnly: boolean;
  crop: number;
  style?: JSX.CSSProperties;
}) {
  let canvas: HTMLCanvasElement | undefined;
  const [src, setSrc] = createSignal<{ crop: number; source: Source }>();
  let frame = 0;

  createEffect(
    () => props.crop,
    (crop) => {
      void source(crop).then((s) => setSrc({ crop, source: s }));
    },
  );

  createEffect(
    () => [props.film, props.recipe, props.filmOnly, src()] as const,
    ([film, recipe, filmOnly, s]) => {
      if (!s || !canvas) return;
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const context = canvas?.getContext('2d');
        if (!context) return;
        const out = context.createImageData(W, H);
        develop(film, recipe, filmOnly, s.source, out);
        context.putImageData(out, 0, 0);
        drawOverlays(context, recipe, filmOnly);
      });
    },
  );

  return (
    <canvas
      ref={(element) => (canvas = element)}
      width={W}
      height={H}
      class="absolute inset-0"
      style={props.style}
    />
  );
}

export function Picture(props: {
  film: Film;
  recipe: RecipeValues;
  filmOnly: boolean;
  crop: number;
  compare?: { film: Film; recipe: RecipeValues; split: number };
}) {
  return (
    <>
      <Canvas film={props.film} recipe={props.recipe} filmOnly={props.filmOnly} crop={props.crop} />
      <Show when={props.compare}>
        {(c) => (
          <Canvas
            film={c().film}
            recipe={c().recipe}
            filmOnly={props.filmOnly}
            crop={props.crop}
            style={{ 'clip-path': `inset(0 0 0 ${c().split}px)` }}
          />
        )}
      </Show>
    </>
  );
}

export function useHistogram() {
  const [histogram, setHistogram] = createSignal<number[]>([]);
  onSettled(() => {
    void source().then((src) => setHistogram(src.histogram));
  });
  return histogram;
}
