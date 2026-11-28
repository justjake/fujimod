import type { CameraKey } from '../../../design/screen';
import { pickerStep } from '../../../design/picker';
import { padStep, padType, padValue, type PadKey } from '../../../design/numberpad';

/* ---- Constants & layout metrics -------------------------------------------------- */

export const W = 720;
export const H = 480;
export const RAIL = 52;
export const SCROLLER = 308;
export const CONTENT = SCROLLER - 12;
export const PICTURE_LEFT = RAIL + SCROLLER;
export const PICTURE_WIDTH = W - PICTURE_LEFT;
export const SPLIT = Math.round((PICTURE_LEFT + W) / 2);

/* ---- Tone curve math ------------------------------------------------------------- */

/** Curve knots, on the curve's 12-bit linear input. */
export const XS = [0, 64, 128, 256, 512, 1024, 1536, 2048, 3072, 4096];

/** Input position on the stops axis, 0..1: knot 1 is -6 EV, the white knot 0 EV. */
export function axis(x: number): number {
  if (x < 64) return Math.max(0, x / 64) / 7;
  return (Math.log2(x / 4096) + 7) / 7;
}
export const US = XS.map(axis);

export const encode = (l: number) => (l <= 0.0031308 ? 12.92 * l : 1.055 * l ** (1 / 2.4) - 0.055);
export const decode = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);

/** Neutral identity knots (encoded sRGB values at each linear X). */
export const NEUTRAL_KNOTS = XS.map((x) => encode(x / 4096));
export const ZERO_TILT = XS.map(() => 0);

/** Where an output value sits on the graph: stops on both axes, so a neutral curve is a diagonal. */
export const disp = (y: number) => axis(decode(Math.max(0, y)) * 4096);
export function fromDisp(d: number): number {
  const c = Math.min(1, Math.max(0, d));
  const x = c < 1 / 7 ? c * 7 * 64 : 4096 * 2 ** (7 * c - 7);
  return encode(x / 4096);
}

export const ZONES = [
  'BLACK',
  'BLACKS',
  'SHADOWS',
  'SHADOWS',
  'MIDTONES',
  'MIDTONES',
  'HIGHLIGHTS',
  'HIGHLIGHTS',
  'WHITES',
  'WHITE',
];
export const KNOT_EV = [
  '',
  '−6 EV',
  '−5 EV',
  '−4 EV',
  '−3 EV',
  '−2 EV',
  '−1.6 EV',
  '−1 EV',
  '−0.4 EV',
  '0 EV',
];
export const BAND_NAMES = ['BLACKS', 'SHADOWS', 'MIDTONES', 'HIGHLIGHTS', 'WHITES'];

/* ---- Film simulation model ------------------------------------------------------- */

export type CurveChannel = 'all' | 'r' | 'g' | 'b';
export type RgbChannel = 'r' | 'g' | 'b';

export interface Primary {
  hue: number;
  sat: number;
}

export type Matrix = number[];

export interface Film {
  base: number;
  knots: number[];
  tilt: number[];
  channelKnots: Record<RgbChannel, number[]>;
  channelTilt: Record<RgbChannel, number[]>;
  saturation: number;
  hue: number;
  warmth: number;
  primaries: Primary[];
  matrix?: Matrix;
  chroma?: number[];
  edges: number[];
  bands: number[];
}

export interface BaseFilmSpec {
  name: string;
  shape: number[];
  saturation: number;
  hue?: number;
  warmth?: number;
  bands?: number[];
}

export const BASES: BaseFilmSpec[] = [
  {
    name: 'PROVIA',
    shape: [0, -0.01, -0.015, -0.012, 0, 0.012, 0.016, 0.012, 0.005, 0],
    saturation: 0,
  },
  {
    name: 'Velvia',
    shape: [0, -0.02, -0.035, -0.03, -0.005, 0.02, 0.028, 0.02, 0.008, 0],
    saturation: 30,
    bands: [20, 10, 0, -10, -30],
  },
  {
    name: 'ASTIA',
    shape: [0, -0.005, -0.01, -0.006, 0, 0.006, 0.008, 0.004, 0, 0],
    saturation: 10,
  },
  {
    name: 'CLASSIC CHR.',
    shape: [0, -0.02, -0.04, -0.035, -0.01, 0.005, 0.0, -0.008, -0.012, 0],
    saturation: -30,
    warmth: -15,
    bands: [-10, 0, 0, -10, -30],
  },
  {
    name: 'PRO Neg. Hi',
    shape: [0, -0.018, -0.025, -0.018, 0, 0.016, 0.02, 0.01, 0.002, 0],
    saturation: -5,
  },
  {
    name: 'PRO Neg. Std',
    shape: [0, 0.01, 0.01, 0.006, 0, -0.004, -0.008, -0.008, -0.004, 0],
    saturation: -12,
  },
  {
    name: 'MONOCHROME',
    shape: [0, -0.015, -0.022, -0.015, 0, 0.012, 0.018, 0.01, 0.004, 0],
    saturation: -100,
  },
  {
    name: 'ETERNA',
    shape: [0.03, 0.04, 0.035, 0.02, 0.005, -0.01, -0.02, -0.025, -0.02, 0],
    saturation: -35,
    bands: [-20, -10, 0, -10, -30],
  },
  {
    name: 'CLASSIC Neg.',
    shape: [0, -0.03, -0.05, -0.04, -0.01, 0.02, 0.025, 0.01, -0.01, 0],
    saturation: -15,
    hue: 4,
    warmth: 10,
  },
];

export const EDGES = [64, 120, 200, 230];

export function baseFilm(base: number): Film {
  const b = BASES[base] ?? BASES[0];
  return {
    base,
    knots: XS.map((x, i) => Math.min(1, Math.max(0, encode(x / 4096) + b.shape[i]))),
    tilt: [...ZERO_TILT],
    channelKnots: {
      r: [...NEUTRAL_KNOTS],
      g: [...NEUTRAL_KNOTS],
      b: [...NEUTRAL_KNOTS],
    },
    channelTilt: {
      r: [...ZERO_TILT],
      g: [...ZERO_TILT],
      b: [...ZERO_TILT],
    },
    saturation: b.saturation,
    hue: b.hue ?? 0,
    warmth: b.warmth ?? 0,
    primaries: [0, 1, 2].map(() => ({ hue: 0, sat: 0 })),
    edges: [...EDGES],
    bands: [...(b.bands ?? [0, 0, 0, 0, 0])],
  };
}

export function cloneFilm(f: Film): Film {
  return {
    ...f,
    knots: [...f.knots],
    tilt: [...f.tilt],
    channelKnots: {
      r: [...f.channelKnots.r],
      g: [...f.channelKnots.g],
      b: [...f.channelKnots.b],
    },
    channelTilt: {
      r: [...f.channelTilt.r],
      g: [...f.channelTilt.g],
      b: [...f.channelTilt.b],
    },
    primaries: f.primaries.map((p) => ({ ...p })),
    matrix: f.matrix ? [...f.matrix] : undefined,
    chroma: f.chroma ? [...f.chroma] : undefined,
    edges: [...f.edges],
    bands: [...f.bands],
  };
}

export function curveKnots(f: Film, ch: CurveChannel): number[] {
  return ch === 'all' ? f.knots : f.channelKnots[ch];
}

export function curveTilt(f: Film, ch: CurveChannel): number[] {
  return ch === 'all' ? f.tilt : f.channelTilt[ch];
}

export function setCurveData(f: Film, ch: CurveChannel, knots: number[], tilt: number[]): Film {
  if (ch === 'all') return { ...f, knots, tilt };
  return {
    ...f,
    channelKnots: { ...f.channelKnots, [ch]: knots },
    channelTilt: { ...f.channelTilt, [ch]: tilt },
  };
}

export function slopesOf(knots: number[], tilt: number[]): number[] {
  return knots.map((_, i) => {
    const a = Math.max(0, i - 1);
    const b = Math.min(9, i + 1);
    return (knots[b] - knots[a]) / (US[b] - US[a]) + tilt[i];
  });
}

export function handlesOf(knots: number[], tilt: number[]): [number, number][] {
  const m = slopesOf(knots, tilt);
  return knots.slice(0, 9).map((y, i) => {
    const du = US[i + 1] - US[i];
    return [y + (m[i] * du) / 3, knots[i + 1] - (m[i + 1] * du) / 3];
  });
}

export function curveEval(knots: number[], hs: [number, number][], u: number): number {
  let i = 0;
  while (i < 8 && u > US[i + 1]) i++;
  const t = Math.min(1, Math.max(0, (u - US[i]) / (US[i + 1] - US[i])));
  const [p1, p2] = hs[i];
  const s = 1 - t;
  return s * s * s * knots[i] + 3 * s * s * t * p1 + 3 * s * t * t * p2 + t * t * t * knots[i + 1];
}

export function primariesMatrix(ps: Primary[]): Matrix {
  const g = 1 / 3;
  const axisN = 1 / Math.sqrt(3);
  const columns = ps.map((p, c) => {
    const chroma = [0, 1, 2].map((k) => (k === c ? 1 : 0) - g);
    const a = (p.hue * Math.PI) / 180;
    const cos = Math.cos(a);
    const sin = Math.sin(a);
    const cross = [
      axisN * (chroma[2] - chroma[1]),
      axisN * (chroma[0] - chroma[2]),
      axisN * (chroma[1] - chroma[0]),
    ];
    const scale = 1 + p.sat / 100;
    return chroma.map((v, k) => (v * cos + cross[k] * sin) * scale);
  });
  const drift = [0, 1, 2].map((k) => columns.reduce((sum, col) => sum + col[k], 0) / 3);
  const m: Matrix = [];
  for (let row = 0; row < 3; row++)
    for (let col = 0; col < 3; col++) m.push(g + columns[col][row] - drift[row]);
  return m;
}

export const mixMatrix = (f: Film): Matrix => f.matrix ?? primariesMatrix(f.primaries);

export function fitPrimaries(m: Matrix): Primary[] {
  const n = 1 / Math.sqrt(3);
  return [0, 1, 2].map((c) => {
    const col = [m[c], m[3 + c], m[6 + c]];
    const mean = (col[0] + col[1] + col[2]) / 3;
    const v = col.map((x) => x - mean);
    const h = [0, 1, 2].map((k) => (k === c ? 1 : 0) - 1 / 3);
    const cross = [h[1] * v[2] - h[2] * v[1], h[2] * v[0] - h[0] * v[2], h[0] * v[1] - h[1] * v[0]];
    const hue =
      (Math.atan2(n * (cross[0] + cross[1] + cross[2]), h[0] * v[0] + h[1] * v[1] + h[2] * v[2]) *
        180) /
      Math.PI;
    const sat = (Math.hypot(v[0], v[1], v[2]) / Math.hypot(h[0], h[1], h[2]) - 1) * 100;
    return { hue: Math.round(hue), sat: Math.round(Math.max(-100, Math.min(100, sat))) };
  });
}

export function chromaMatrix(f: Film): number[] {
  if (f.chroma) return f.chroma;
  const s = 1 + f.saturation / 100;
  const a = (f.hue * Math.PI) / 180;
  const dx = -0.6;
  const dy = 0.8;
  const w = f.warmth / 100;
  const k00 = 1 + w * dx * dx;
  const k01 = w * dx * dy;
  const k11 = 1 + w * dy * dy;
  const c = Math.cos(a) * s;
  const sn = Math.sin(a) * s;
  return [c * k00 - sn * k01, c * k01 - sn * k11, sn * k00 + c * k01, sn * k01 + c * k11];
}

export function bandGains(f: Film): Float32Array {
  const band = new Float32Array(256);
  const centers = [0, ...f.edges, 255];
  for (let y = 0; y < 256; y++) {
    let i = 0;
    while (i < 4 && y > f.edges[i]) i++;
    const mid = (centers[i] + centers[i + 1]) / 2;
    const next = y < mid ? Math.max(0, i - 1) : Math.min(4, i + 1);
    const nextMid = (centers[next] + centers[next + 1]) / 2;
    const t =
      next === i ? 0 : Math.min(0.5, Math.abs(y - mid) / Math.max(1, Math.abs(nextMid - mid)));
    band[y] = 1 + ((1 - t) * f.bands[i] + t * f.bands[next]) / 100;
  }
  return band;
}

/* ---- Film catalog (stock + SD card films) ---------------------------------------- */

export interface FilmEntry {
  id: string;
  name: string;
  short: string;
  icon: string;
  stockIcon?: number;
  isCard: boolean;
  from?: string;
  film: Film;
}

function makeLeicaX(): Film {
  const f = baseFilm(3);
  f.knots[0] += 0.035;
  f.knots[1] += 0.025;
  f.knots[3] -= 0.015;
  f.knots[7] += 0.022;
  f.channelKnots.r[6] += 0.018;
  f.channelKnots.r[7] += 0.022;
  f.channelKnots.b[2] += 0.015;
  f.channelKnots.b[7] -= 0.02;
  f.saturation = -22;
  f.warmth = 10;
  f.primaries = [
    { hue: 4, sat: -8 },
    { hue: -10, sat: -22 },
    { hue: 2, sat: 6 },
  ];
  f.bands = [-15, 0, 5, -10, -35];
  return f;
}

function makeAcros(): Film {
  const f = baseFilm(6);
  f.knots[0] = 0;
  f.knots[1] -= 0.012;
  f.knots[2] -= 0.024;
  f.knots[6] += 0.022;
  f.knots[7] += 0.028;
  f.saturation = -100;
  return f;
}

export function initialFilmCatalog(): FilmEntry[] {
  return [
    {
      id: 'provia',
      name: 'PROVIA/STANDARD',
      short: 'PROVIA',
      icon: 'provia',
      stockIcon: 0x1dc,
      isCard: false,
      film: baseFilm(0),
    },
    {
      id: 'velvia',
      name: 'Velvia/VIVID',
      short: 'Velvia',
      icon: 'velvia',
      stockIcon: 0x1dd,
      isCard: false,
      film: baseFilm(1),
    },
    {
      id: 'astia',
      name: 'ASTIA/SOFT',
      short: 'ASTIA',
      icon: 'astia',
      stockIcon: 0x1de,
      isCard: false,
      film: baseFilm(2),
    },
    {
      id: 'classic-chrome',
      name: 'CLASSIC CHROME',
      short: 'CLASSIC CHR.',
      icon: 'classic-chrome',
      stockIcon: 0x1e5,
      isCard: false,
      film: baseFilm(3),
    },
    {
      id: 'pro-neg-hi',
      name: 'PRO Neg. Hi',
      short: 'PRO Neg. Hi',
      icon: 'pro-neg-hi',
      stockIcon: 0x1df,
      isCard: false,
      film: baseFilm(4),
    },
    {
      id: 'pro-neg-std',
      name: 'PRO Neg. Std',
      short: 'PRO Neg. Std',
      icon: 'pro-neg-std',
      stockIcon: 0x1e0,
      isCard: false,
      film: baseFilm(5),
    },
    {
      id: 'mono',
      name: 'MONOCHROME',
      short: 'MONO',
      icon: 'mono',
      stockIcon: 0x1e1,
      isCard: false,
      film: baseFilm(6),
    },
    {
      id: 'leica-x',
      name: 'LEICA X',
      short: 'LEICA X',
      icon: 'leica-x',
      isCard: true,
      from: 'CLASSIC CHR.',
      film: makeLeicaX(),
    },
    {
      id: 'acros',
      name: 'ACROS',
      short: 'ACROS',
      icon: 'acros',
      isCard: true,
      from: 'MONOCHROME',
      film: makeAcros(),
    },
  ];
}

export const FILM_ICONS = [
  'provia',
  'velvia',
  'astia',
  'classic-chrome',
  'pro-neg-hi',
  'pro-neg-std',
  'eterna',
  'bleach-bypass',
  'classic-neg',
  'nostalgic-neg',
  'reala-ace',
  'neutral',
  'acros',
  'acros-ye',
  'acros-r',
  'acros-g',
  'mono',
  'mono-ye',
  'mono-r',
  'mono-g',
  'sepia',
  'leica-x',
  'custom',
  'text:',
];
export const ICON_COLUMNS = 6;
export const ICON_CELL = 60;
export const MONOGRAM_LIMIT = 4;

/* ---- Film tools & groups --------------------------------------------------------- */

export type FilmTool = 'curve' | 'chroma' | 'mix' | 'luma';
export const FILM_TOOLS: FilmTool[] = ['curve', 'chroma', 'mix', 'luma'];

export const FILM_TOOL_NAMES: Record<FilmTool, string> = {
  curve: 'TONE CURVE',
  chroma: 'CHROMA',
  mix: 'COLOR MIX',
  luma: 'LUMA SAT',
};

export const FILM_GROUPS: Record<FilmTool, (f: Film) => unknown> = {
  curve: (f) => [f.knots, f.tilt, f.channelKnots, f.channelTilt],
  chroma: (f) => [f.saturation, f.hue, f.warmth, f.chroma],
  mix: (f) => [f.primaries, f.matrix],
  luma: (f) => [f.bands, f.edges],
};

export const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

export function groupSource(f: Film, tool: FilmTool): number | undefined {
  const value = FILM_GROUPS[tool](f);
  if (same(FILM_GROUPS[tool](baseFilm(f.base)), value)) return f.base;
  const k = BASES.findIndex((_, i) => same(FILM_GROUPS[tool](baseFilm(i)), value));
  return k < 0 ? undefined : k;
}

export function copyGroup(f: Film, tool: FilmTool, k: number): Film {
  const b = baseFilm(k);
  switch (tool) {
    case 'curve':
      return {
        ...f,
        knots: b.knots,
        tilt: b.tilt,
        channelKnots: b.channelKnots,
        channelTilt: b.channelTilt,
      };
    case 'chroma':
      return { ...f, saturation: b.saturation, hue: b.hue, warmth: b.warmth, chroma: undefined };
    case 'mix':
      return { ...f, primaries: b.primaries, matrix: undefined };
    case 'luma':
      return { ...f, bands: b.bands, edges: b.edges };
  }
}

export interface Scale {
  name: string;
  get: (f: Film) => number;
  put: (f: Film, v: number) => Film;
  min: number;
  max: number;
  step: number;
  track: string;
}

const gradient = (...stops: string[]) => `linear-gradient(to right, ${stops.join(', ')})`;
const GRAY = '#8e8e93';

export const CHROMA_SCALES: Scale[] = [
  {
    name: 'SATURATION',
    get: (f) => f.saturation,
    put: (f, v) => ({ ...f, saturation: v, chroma: undefined }),
    min: -100,
    max: 100,
    step: 5,
    track: gradient(GRAY, '#ff9500'),
  },
  {
    name: 'HUE',
    get: (f) => f.hue,
    put: (f, v) => ({ ...f, hue: v, chroma: undefined }),
    min: -180,
    max: 180,
    step: 1,
    track: gradient(
      '#00c7be',
      '#0a84ff',
      '#bf5af2',
      '#ff375f',
      '#ff9f0a',
      '#ffd60a',
      '#30d158',
      '#00c7be',
    ),
  },
  {
    name: 'WARM/COOL',
    get: (f) => f.warmth,
    put: (f, v) => ({ ...f, warmth: v, chroma: undefined }),
    min: -100,
    max: 100,
    step: 5,
    track: gradient('#0a84ff', GRAY, '#ff9f0a'),
  },
];

export const MIX = [
  {
    name: 'REDS',
    color: '#ff3b30',
    linear: [0.75, 0.06, 0.04],
    hue: gradient('#ff2d78', '#ff3b30', '#ff9500'),
  },
  {
    name: 'GREENS',
    color: '#34c759',
    linear: [0.08, 0.5, 0.1],
    hue: gradient('#b5d334', '#34c759', '#00c7be'),
  },
  {
    name: 'BLUES',
    color: '#0a64ff',
    linear: [0.04, 0.1, 0.7],
    hue: gradient('#32ade6', '#0a64ff', '#a35bff'),
  },
];

export const mixScales = (p: number): Scale[] => [
  {
    name: 'HUE',
    get: (f) => f.primaries[p].hue,
    put: (f, v) => ({
      ...f,
      matrix: undefined,
      primaries: f.primaries.map((q, i) => (i === p ? { ...q, hue: v } : q)),
    }),
    min: -180,
    max: 180,
    step: 2,
    track: MIX[p].hue,
  },
  {
    name: 'SATURATION',
    get: (f) => f.primaries[p].sat,
    put: (f, v) => ({
      ...f,
      matrix: undefined,
      primaries: f.primaries.map((q, i) => (i === p ? { ...q, sat: v } : q)),
    }),
    min: -100,
    max: 100,
    step: 5,
    track: gradient(GRAY, MIX[p].color),
  },
];

export const MATRIX_SIZE = { mix: 3, chroma: 2 } as const;
export const matrixCells = (f: Film, tool: 'mix' | 'chroma'): number[] =>
  tool === 'mix' ? mixMatrix(f) : chromaMatrix(f);
export const fixedCell = (tool: 'mix' | 'chroma', cell: number) => tool === 'mix' && cell % 4 === 0;

export function putMatrixCell(f: Film, tool: 'mix' | 'chroma', cell: number, v: number): Film {
  const m = [...matrixCells(f, tool)];
  m[cell] = Math.round(v * 100) / 100;
  if (tool === 'chroma') {
    const hue = Math.round((Math.atan2(m[2], m[0]) * 180) / Math.PI);
    const saturation = Math.round((Math.hypot(m[0], m[2]) - 1) * 100);
    return { ...f, chroma: m, hue, saturation: Math.max(-100, Math.min(100, saturation)) };
  }
  const row = Math.floor(cell / 3) * 3;
  const diag = row + row / 3;
  m[diag] =
    Math.round(
      (1 - m.slice(row, row + 3).reduce((sum, x, k) => (row + k === diag ? sum : sum + x), 0)) *
        100,
    ) / 100;
  return { ...f, matrix: m, primaries: fitPrimaries(m) };
}

/* ---- Recipe settings (Light, Color, Detail, Effects) ----------------------------- */

export type Step = number | string;
export const LEVELS: Step[] = [-2, -1, 0, 1, 2];
export const CLARITY_STEPS: Step[] = [-5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5];
export const NR_LEVELS: Step[] = ['OFF', 'SNR', 'CNR', 'LNR', -2, -1, 0, 1, 2];

export const STEP_NAMES: Record<string, string> = {
  OFF: 'NR OFF',
  SNR: 'SPECKLE NR ONLY',
  CNR: 'COLOR NR LOW',
  LNR: 'LUMA NR OFF',
};

export const fmtStep = (n: Step) =>
  typeof n === 'string' ? n : n > 0 ? `+${n}` : n < 0 ? `\u2212${-n}` : '0';

export interface ChoiceOption {
  name: string;
  icon?: number;
}

export type RowIcon = { stock: number } | { mod: string } | { word: string; badge?: string };

export type SettingGroupId = 'light' | 'color' | 'detail' | 'effects';

export interface SliderSetting {
  kind: 'slider';
  id: string;
  group: SettingGroupId;
  name: string;
  icon: RowIcon;
  continues?: boolean;
  steps: Step[];
}

export interface ChoiceSetting {
  kind: 'choice';
  id: string;
  group: SettingGroupId;
  name: string;
  icon: RowIcon;
  continues?: boolean;
  choices: ChoiceOption[];
}

export type RecipeSetting = SliderSetting | ChoiceSetting;

const WB_CHOICES: ChoiceOption[] = [
  { name: 'AUTO', icon: 0x1d2 },
  { name: 'CUSTOM 1', icon: 0x1d3 },
  { name: 'COLOR TEMPERATURE', icon: 0x1d4 },
  { name: 'FINE', icon: 0x1d5 },
  { name: 'SHADE', icon: 0x1d6 },
  { name: 'FLUORESCENT LIGHT-1', icon: 0x1d7 },
  { name: 'FLUORESCENT LIGHT-2', icon: 0x1d8 },
  { name: 'FLUORESCENT LIGHT-3', icon: 0x1d9 },
  { name: 'INCANDESCENT LIGHT', icon: 0x1da },
  { name: 'UNDERWATER', icon: 0x1db },
];

const DR_CHOICES: ChoiceOption[] = [
  { name: 'AUTO', icon: 0x575 },
  { name: 'DR100', icon: 0x572 },
  { name: 'DR200', icon: 0x573 },
  { name: 'DR400', icon: 0x574 },
];

const PRIORITY_CHOICES: ChoiceOption[] = [
  { name: 'OFF' },
  { name: 'WEAK' },
  { name: 'STRONG' },
  { name: 'AUTO' },
];
const EFFECT_CHOICES: ChoiceOption[] = [{ name: 'OFF' }, { name: 'WEAK' }, { name: 'STRONG' }];
const GRAIN_SIZE_CHOICES: ChoiceOption[] = [{ name: 'SMALL' }, { name: 'LARGE' }];
const BORDER_CHOICES: ChoiceOption[] = [{ name: 'OFF' }, { name: 'THIN' }, { name: 'CLASSIC' }];
const ON_OFF_CHOICES: ChoiceOption[] = [{ name: 'OFF' }, { name: 'ON' }];

export const RECIPE_SETTINGS: RecipeSetting[] = [
  // LIGHT
  {
    kind: 'choice',
    id: 'dr',
    group: 'light',
    name: 'DYNAMIC RANGE',
    icon: { stock: 0x5f1 },
    choices: DR_CHOICES,
  },
  {
    kind: 'choice',
    id: 'priority',
    group: 'light',
    name: 'D RANGE PRIORITY',
    icon: { mod: 'd_range_priority' },
    choices: PRIORITY_CHOICES,
  },
  {
    kind: 'slider',
    id: 'highlight',
    group: 'light',
    name: 'HIGHLIGHT TONE',
    icon: { stock: 0x5f6 },
    steps: LEVELS,
  },
  {
    kind: 'slider',
    id: 'shadow',
    group: 'light',
    name: 'SHADOW TONE',
    icon: { stock: 0x607 },
    steps: LEVELS,
  },
  // COLOR
  {
    kind: 'choice',
    id: 'wb',
    group: 'color',
    name: 'WHITE BALANCE',
    icon: { stock: 0x609 },
    choices: WB_CHOICES,
  },
  {
    kind: 'slider',
    id: 'color',
    group: 'color',
    name: 'COLOR',
    icon: { stock: 0x5f0 },
    steps: LEVELS,
  },
  {
    kind: 'choice',
    id: 'chrome',
    group: 'color',
    name: 'COLOR CHROME EFFECT',
    icon: { mod: 'color_chrome' },
    choices: EFFECT_CHOICES,
  },
  {
    kind: 'choice',
    id: 'fxblue',
    group: 'color',
    name: 'COLOR CHROME FX BLUE',
    icon: { mod: 'color_chrome_blue' },
    choices: EFFECT_CHOICES,
  },
  // DETAIL
  {
    kind: 'slider',
    id: 'sharp',
    group: 'detail',
    name: 'SHARPNESS',
    icon: { stock: 0x605 },
    steps: LEVELS,
  },
  {
    kind: 'slider',
    id: 'clarity',
    group: 'detail',
    name: 'CLARITY',
    icon: { mod: 'clarity' },
    steps: CLARITY_STEPS,
  },
  {
    kind: 'slider',
    id: 'nr',
    group: 'detail',
    name: 'NOISE REDUCTION',
    icon: { stock: 0x603 },
    steps: NR_LEVELS,
  },
  // EFFECTS
  {
    kind: 'choice',
    id: 'grain',
    group: 'effects',
    name: 'GRAIN EFFECT',
    icon: { mod: 'film_grain' },
    choices: EFFECT_CHOICES,
  },
  {
    kind: 'choice',
    id: 'grainSize',
    group: 'effects',
    name: 'GRAIN SIZE',
    icon: { mod: 'film_grain' },
    continues: true,
    choices: GRAIN_SIZE_CHOICES,
  },
  {
    kind: 'choice',
    id: 'halation',
    group: 'effects',
    name: 'HALATION',
    icon: { mod: 'halation' },
    choices: EFFECT_CHOICES,
  },
  {
    kind: 'choice',
    id: 'mist',
    group: 'effects',
    name: 'MIST FILTER',
    icon: { word: 'Mist' },
    choices: EFFECT_CHOICES,
  },
  {
    kind: 'choice',
    id: 'leaks',
    group: 'effects',
    name: 'LIGHT LEAKS',
    icon: { mod: 'light_leaks' },
    choices: EFFECT_CHOICES,
  },
  {
    kind: 'choice',
    id: 'borders',
    group: 'effects',
    name: 'FILM BORDERS',
    icon: { mod: 'borders' },
    choices: BORDER_CHOICES,
  },
  {
    kind: 'choice',
    id: 'dateStamp',
    group: 'effects',
    name: 'DATE STAMP',
    icon: { mod: 'date_stamp' },
    choices: ON_OFF_CHOICES,
  },
];

export const SETTING_GROUPS: SettingGroupId[] = ['light', 'color', 'detail', 'effects'];
export const SETTING_GROUP_TITLES: Record<SettingGroupId, string> = {
  light: 'LIGHT',
  color: 'COLOR',
  detail: 'DETAIL',
  effects: 'EFFECTS',
};

export const ICON_WORDS: Record<string, string> = {
  [0x5f3]: 'Film',
  [0x609]: 'WB',
  [0x5f1]: 'DR',
  [0x5f6]: 'H',
  [0x607]: 'S',
  [0x5f0]: 'Color',
  [0x605]: 'Sharp',
  [0x603]: 'NR',
  d_range_priority: 'D-Pri',
  clarity: 'Clarity',
  film_grain: 'Grain',
  color_chrome: 'CCR',
  color_chrome_blue: 'CCB',
};

export type RecipeValues = Record<string, number> & { shiftR: number; shiftB: number };

const at = (steps: Step[], value: Step) => steps.indexOf(value);

export const NEUTRAL_RECIPE: RecipeValues = {
  dr: 1,
  priority: 0,
  highlight: at(LEVELS, 0),
  shadow: at(LEVELS, 0),
  wb: 0,
  color: at(LEVELS, 0),
  chrome: 0,
  fxblue: 0,
  sharp: at(LEVELS, 0),
  clarity: at(CLARITY_STEPS, 0),
  nr: at(NR_LEVELS, 0),
  grain: 0,
  grainSize: 0,
  halation: 0,
  mist: 0,
  leaks: 0,
  borders: 0,
  dateStamp: 0,
  shiftR: 0,
  shiftB: 0,
};

export const BASE_RECIPE: RecipeValues = {
  dr: 0,
  priority: 0,
  highlight: at(LEVELS, -1),
  shadow: at(LEVELS, 1),
  wb: 0,
  color: at(LEVELS, 1),
  chrome: 1,
  fxblue: 0,
  sharp: at(LEVELS, -1),
  clarity: at(CLARITY_STEPS, 0),
  nr: at(NR_LEVELS, -2),
  grain: 1,
  grainSize: 0,
  halation: 0,
  mist: 0,
  leaks: 0,
  borders: 0,
  dateStamp: 0,
  shiftR: 2,
  shiftB: -3,
};

export function settingValueName(setting: RecipeSetting, recipe: RecipeValues): string {
  const idx = recipe[setting.id] ?? 0;
  if (setting.kind === 'choice') return setting.choices[idx]?.name ?? '';
  const step = setting.steps[idx];
  return STEP_NAMES[String(step)] ?? fmtStep(step);
}

/* ---- RAW conversion -------------------------------------------------------------- */

export const RAWS = [
  { name: 'DSCF0412.RAF', zoom: 1.8, x: 0.62, y: 0.42, info: 'f/2.8 · 1/500 · ISO 200' },
  { name: 'DSCF0413.RAF', zoom: 2.2, x: 0.18, y: 0.5, info: 'f/4 · 1/250 · ISO 400' },
  { name: 'DSCF0415.RAF', zoom: 1.5, x: 0.4, y: 0.2, info: 'f/5.6 · 1/1000 · ISO 200' },
];

export const RAW_OPTIONS = [
  {
    key: 'push',
    name: 'PUSH/PULL PROCESSING',
    short: 'PUSH/PULL',
    values: [
      '−1 EV',
      '−2/3 EV',
      '−1/3 EV',
      '0',
      '+1/3 EV',
      '+2/3 EV',
      '+1 EV',
      '+1 1/3 EV',
      '+1 2/3 EV',
      '+2 EV',
      '+2 1/3 EV',
      '+2 2/3 EV',
      '+3 EV',
    ],
    start: 3,
  },
  {
    key: 'space',
    name: 'COLOR SPACE',
    short: 'COLOR SPACE',
    values: ['sRGB', 'Adobe RGB'],
    start: 0,
  },
  {
    key: 'filetype',
    name: 'FILE TYPE',
    short: 'FILE TYPE',
    values: ['JPEG', 'HEIF', 'TIFF 8BIT', 'TIFF 16BIT'],
    start: 0,
  },
  {
    key: 'size',
    name: 'IMAGE SIZE',
    short: 'IMAGE SIZE',
    values: ['L 3:2', 'M 3:2', 'S 3:2'],
    start: 0,
  },
  { key: 'quality', name: 'IMAGE QUALITY', short: 'QUALITY', values: ['FINE', 'NORMAL'], start: 0 },
  { key: 'lmo', name: 'LENS MODULATION OPTIMIZER', short: 'LMO', values: ['ON', 'OFF'], start: 0 },
] as const;

export const RAW_DEFAULT: number[] = RAW_OPTIONS.map((o) => o.start);

/* ---- Saved recipes on SD card ---------------------------------------------------- */

export interface SavedRecipe {
  slot: string;
  name: string;
  author: string;
  filmId: string;
  recipe: RecipeValues;
}

export const CARD_RECIPES: SavedRecipe[] = [
  {
    slot: 'C1',
    name: 'KODAK GOLD 200',
    author: 'FujiXWeekly',
    filmId: 'classic-chrome',
    recipe: { ...BASE_RECIPE },
  },
  {
    slot: 'C2',
    name: 'LEICA STREET',
    author: 'Community',
    filmId: 'leica-x',
    recipe: {
      ...BASE_RECIPE,
      shiftR: 1,
      shiftB: -1,
      highlight: at(LEVELS, 0),
      shadow: at(LEVELS, 2),
      grain: 1,
      halation: 1,
    },
  },
  {
    slot: 'C3',
    name: 'TRI-X 400',
    author: 'FujiXWeekly',
    filmId: 'acros',
    recipe: {
      ...BASE_RECIPE,
      highlight: at(LEVELS, 1),
      shadow: at(LEVELS, 2),
      grain: 2,
      grainSize: 1,
      shiftR: 0,
      shiftB: 0,
    },
  },
];

/* ---- Sections & unified focus model ---------------------------------------------- */

export type TopSection = 'raw' | 'recipe' | 'film' | 'light' | 'color' | 'detail' | 'effects';
export type RailId = TopSection | FilmTool;

export type FocusTarget =
  | { kind: 'rail'; id: RailId }
  | { kind: 'raw'; item: 'strip' | 'reflect' | 'create' | number }
  | { kind: 'recipe'; item: 'name' | 'save' | 'saveas' | 'open' }
  | { kind: 'film-header' }
  | { kind: 'film-tiles' }
  | { kind: 'film-action'; action: 'update' | 'saveas' | 'reset' }
  | { kind: 'film-tool-header'; tool: FilmTool }
  | { kind: 'film-curve' }
  | { kind: 'film-chroma'; row: 0 | 1 | 2 | 3 }
  | { kind: 'film-mix'; row: 'chips' | 0 | 1 | 'numbers' }
  | { kind: 'film-luma' }
  | { kind: 'setting'; id: string };

export type PickerState =
  | { kind: 'copy'; tool: FilmTool; at: number }
  | { kind: 'open-recipe'; at: number }
  | { kind: 'compare'; at: number }
  | { kind: 'setting-list'; settingId: string; at: number }
  | { kind: 'raw-option'; index: number; at: number };

export type TypingTarget = 'recipe-name' | 'recipe-saveas' | 'film-saveas';

export interface Typing {
  text: string;
  cursor: number;
  focus: number;
  shift: 'off' | 'once' | 'locked';
  digits: boolean;
  target: TypingTarget;
  /** When saving a new film from the film-switch prompt, switch to this film id after saving. */
  switchAfterId?: string;
}

export interface Pad {
  tool: 'chroma' | 'mix';
  cell?: number;
  scaleIndex?: number;
  text: string;
  fresh: boolean;
  at: number;
}

export interface FilmSwitchModalState {
  targetFilmId: string;
  focus: number;
}

export interface UnifiedState {
  mode: 'live' | 'raw';
  /** Active RAW frame (1..RAWS.length) when mode === 'raw'. */
  crop: number;
  raw: number[];
  created: number;

  /** Recipe metadata & values. */
  slot: string;
  recipeName: string;
  recipeAuthor: string;
  recipe: RecipeValues;
  savedRecipe: RecipeValues;
  savedFilmId: string;

  /** Film simulation catalog & active film. */
  films: FilmEntry[];
  filmId: string;
  /** Local film simulation edits; compared against the active FilmEntry.film to detect dirty state. */
  film: Film;

  /** Section collapse/expand state. Film simulation starts collapsed in casual view. */
  collapsed: Record<TopSection, boolean>;

  /** Film ONLY isolation toggle: renders neutral recipe settings and dims Light/Color/Detail/Effects. */
  filmOnly: boolean;

  /** Film tool interactive state. */
  curveChannel: CurveChannel;
  knot: number;
  bend: boolean;
  band: number;
  inside: boolean;
  fullCurve: boolean;
  primary: number;

  /** Focus & overlays. */
  focus: FocusTarget;
  wbShift: { open: boolean; from?: [number, number] };
  numbers: { tool: 'mix' | 'chroma'; cell: number } | null;
  pad: Pad | null;
  picker: PickerState | null;
  keyboard: Typing | null;
  switchModal: FilmSwitchModalState | null;
  reflectModal: { focus: number } | null;

  /** Compare A|B. */
  compare: { b: 'opened' | number; split: number } | null;
  /** Optional scroll anchor for review states. */
  scrollTo?: RailId;
}

/* ---- Helpers on UnifiedState ----------------------------------------------------- */

export function activeFilmEntry(s: UnifiedState): FilmEntry {
  return s.films.find((f) => f.id === s.filmId) ?? s.films[0];
}

export function isFilmDirty(s: UnifiedState): boolean {
  return !same(s.film, activeFilmEntry(s).film);
}

export function isRecipeDirty(s: UnifiedState): boolean {
  return !same(s.recipe, s.savedRecipe) || s.filmId !== s.savedFilmId || isFilmDirty(s);
}

export function isFilmToolEdited(s: UnifiedState, tool: FilmTool): boolean {
  return !same(FILM_GROUPS[tool](s.film), FILM_GROUPS[tool](activeFilmEntry(s).film));
}

export function isInFilmArea(focus: FocusTarget): boolean {
  switch (focus.kind) {
    case 'film-header':
    case 'film-tiles':
    case 'film-action':
    case 'film-tool-header':
    case 'film-curve':
    case 'film-chroma':
    case 'film-mix':
    case 'film-luma':
      return true;
    case 'rail':
      return focus.id === 'film' || FILM_TOOLS.includes(focus.id as FilmTool);
    default:
      return false;
  }
}

/** The rail's section that holds a focus: the film tools are inside FILM. */
export function sectionOfFocus(focus: FocusTarget): TopSection {
  switch (focus.kind) {
    case 'rail':
      return (FILM_TOOLS as RailId[]).includes(focus.id) ? 'film' : (focus.id as TopSection);
    case 'raw':
      return 'raw';
    case 'recipe':
      return 'recipe';
    case 'film-header':
    case 'film-tiles':
    case 'film-action':
    case 'film-tool-header':
    case 'film-curve':
    case 'film-chroma':
    case 'film-mix':
    case 'film-luma':
      return 'film';
    case 'setting': {
      const setting = RECIPE_SETTINGS.find((r) => r.id === focus.id);
      return setting?.group ?? 'light';
    }
  }
}

export function visibleTopSections(s: UnifiedState): TopSection[] {
  return s.mode === 'raw'
    ? ['raw', 'recipe', 'film', 'light', 'color', 'detail', 'effects']
    : ['recipe', 'film', 'light', 'color', 'detail', 'effects'];
}

export function nextName(name: string): string {
  const match = /^(.*?)(?: (\d+))?$/.exec(name);
  return `${match?.[1] ?? name} ${Number(match?.[2] ?? 1) + 1}`;
}

export const typing = (text: string, target: TypingTarget, switchAfterId?: string): Typing => ({
  text,
  cursor: text.length,
  focus: 4,
  shift: 'off',
  digits: false,
  target,
  switchAfterId,
});

/* ---- Initial state & review presets ---------------------------------------------- */

export function createInitialState(over: Partial<UnifiedState> = {}): UnifiedState {
  const films = over.films ?? initialFilmCatalog();
  const filmId = over.filmId ?? 'classic-chrome';
  const entry = films.find((f) => f.id === filmId) ?? films[3];
  const recipe = over.recipe ? { ...over.recipe } : { ...BASE_RECIPE };
  return {
    mode: 'live',
    crop: 1,
    raw: [...RAW_DEFAULT],
    created: 0,
    slot: 'C1',
    recipeName: 'KODAK GOLD 200',
    recipeAuthor: 'FujiXWeekly',
    recipe,
    savedRecipe: { ...BASE_RECIPE },
    savedFilmId: filmId,
    films,
    filmId,
    film: over.film ? cloneFilm(over.film) : cloneFilm(entry.film),
    collapsed: {
      raw: false,
      recipe: false,
      film: true,
      light: false,
      color: false,
      detail: false,
      effects: false,
    },
    filmOnly: false,
    curveChannel: 'all',
    knot: 4,
    bend: false,
    band: 1,
    inside: false,
    fullCurve: false,
    primary: 0,
    focus: { kind: 'film-tiles' },
    wbShift: { open: false },
    numbers: null,
    pad: null,
    picker: null,
    keyboard: null,
    switchModal: null,
    reflectModal: null,
    compare: null,
    ...over,
  };
}

/** An edited LEICA X film with distinct R/G/B curve separation for review states. */
export function createEditedLeicaFilm(): Film {
  const films = initialFilmCatalog();
  const leica = films.find((f) => f.id === 'leica-x')!;
  const f = cloneFilm(leica.film);
  f.knots[1] += 0.03;
  f.knots[6] += 0.025;
  f.channelKnots.r[5] += 0.03;
  f.channelKnots.r[7] += 0.035;
  f.channelKnots.g[4] -= 0.012;
  f.channelKnots.b[2] += 0.032;
  f.channelKnots.b[7] -= 0.03;
  f.saturation = -14;
  return f;
}

/* ---- Film switching & file actions ----------------------------------------------- */

export function requestFilmSelect(s: UnifiedState, targetFilmId: string): UnifiedState {
  if (targetFilmId === s.filmId) return s;
  if (isFilmDirty(s)) {
    return { ...s, switchModal: { targetFilmId, focus: 0 } };
  }
  const entry = s.films.find((f) => f.id === targetFilmId);
  if (!entry) return s;
  return {
    ...s,
    filmId: entry.id,
    film: cloneFilm(entry.film),
    switchModal: null,
    inside: false,
  };
}

export function updateCardFilm(s: UnifiedState): UnifiedState {
  const entry = activeFilmEntry(s);
  if (!entry.isCard) return s;
  const films = s.films.map((f) => (f.id === entry.id ? { ...f, film: cloneFilm(s.film) } : f));
  return { ...s, films };
}

export function resetFilmEdits(s: UnifiedState): UnifiedState {
  const entry = activeFilmEntry(s);
  return { ...s, film: cloneFilm(entry.film), inside: false, bend: false };
}

export function openSaveFilmKeyboard(s: UnifiedState, switchAfterId?: string): UnifiedState {
  const entry = activeFilmEntry(s);
  const suggested = entry.isCard ? nextName(entry.short) : `${entry.short} 2`;
  return {
    ...s,
    switchModal: null,
    picker: null,
    keyboard: typing(suggested, 'film-saveas', switchAfterId),
  };
}

function commitNewFilm(s: UnifiedState, name: string, switchAfterId?: string): UnifiedState {
  const trimmed = name.trim() || 'CUSTOM FILM';
  const id = `card-${trimmed.toLowerCase().replace(/[^a-z0-9]+/g, '-')}-${s.films.length}`;
  const entry = activeFilmEntry(s);
  const created: FilmEntry = {
    id,
    name: trimmed,
    short: trimmed,
    icon: `text:${trimmed.replace(/\s+/g, '').slice(0, 3).toUpperCase()}`,
    isCard: true,
    from: entry.short,
    film: cloneFilm(s.film),
  };
  const films = [...s.films, created];
  if (switchAfterId) {
    const target = films.find((f) => f.id === switchAfterId) ?? created;
    return {
      ...s,
      films,
      filmId: target.id,
      film: cloneFilm(target.film),
      keyboard: null,
    };
  }
  return {
    ...s,
    films,
    filmId: created.id,
    film: cloneFilm(created.film),
    keyboard: null,
  };
}

export function switchModalOptions(
  s: UnifiedState,
): { id: 'update' | 'saveas' | 'discard' | 'cancel'; label: string }[] {
  const entry = activeFilmEntry(s);
  const opts: { id: 'update' | 'saveas' | 'discard' | 'cancel'; label: string }[] = [];
  if (entry.isCard) {
    opts.push({ id: 'update', label: `UPDATE ${entry.short}` });
  }
  opts.push({ id: 'saveas', label: 'SAVE AS NEW FILM' });
  opts.push({ id: 'discard', label: 'DISCARD' });
  opts.push({ id: 'cancel', label: 'CANCEL' });
  return opts;
}

export function applySwitchModalChoice(
  s: UnifiedState,
  choice: 'update' | 'saveas' | 'discard' | 'cancel',
): UnifiedState {
  const modal = s.switchModal;
  if (!modal) return s;
  if (choice === 'cancel') return { ...s, switchModal: null };
  if (choice === 'saveas') return openSaveFilmKeyboard(s, modal.targetFilmId);
  let next = s;
  if (choice === 'update') {
    next = updateCardFilm(s);
  }
  const target = next.films.find((f) => f.id === modal.targetFilmId);
  if (!target) return { ...next, switchModal: null };
  return {
    ...next,
    filmId: target.id,
    film: cloneFilm(target.film),
    switchModal: null,
    inside: false,
  };
}

export function requestReflectShootingCond(s: UnifiedState): UnifiedState {
  const hasRawEdits = s.raw.some((v, i) => v !== RAW_DEFAULT[i]);
  if (isRecipeDirty(s) || isFilmDirty(s) || hasRawEdits) {
    return { ...s, reflectModal: { focus: 0 } };
  }
  return { ...s, raw: [...RAW_DEFAULT], recipe: { ...BASE_RECIPE }, reflectModal: null };
}

export function applyReflectModalChoice(s: UnifiedState, replace: boolean): UnifiedState {
  if (!replace) return { ...s, reflectModal: null };
  const baseEntry = activeFilmEntry(s);
  return {
    ...s,
    raw: [...RAW_DEFAULT],
    recipe: { ...BASE_RECIPE },
    film: cloneFilm(baseEntry.film),
    reflectModal: null,
  };
}

/* ---- Keyboard & NumberPad -------------------------------------------------------- */

export type KeyKind =
  | 'char'
  | 'shift'
  | 'delete'
  | 'layer'
  | 'space'
  | 'return'
  | 'caret-left'
  | 'caret-right';
export type KeyCap = { kind: KeyKind; row: number; left: number; width: number; label: string };

export function keyCaps(t: Typing): KeyCap[] {
  const rows = t.digits
    ? ['1234567890', "-/:;()&@'", '.,?!+#*']
    : ['qwertyuiop', 'asdfghjkl', 'zxcvbnm'];
  const caps: KeyCap[] = [
    { kind: 'caret-left', row: 0, left: 0, width: 15, label: '' },
    { kind: 'caret-right', row: 0, left: 85, width: 15, label: '' },
  ];
  rows.forEach((chars, r) => {
    const row = r + 1;
    const first =
      row === 2 ? 5 + (10 - chars.length) * 5 + (chars.length === 9 ? -5 : 0) : row === 3 ? 15 : 0;
    if (row === 3) caps.push({ kind: 'shift', row, left: 0, width: 15, label: '' });
    Array.from(chars).forEach((c, i) =>
      caps.push({
        kind: 'char',
        row,
        left: first + 10 * i,
        width: 10,
        label: !t.digits && t.shift !== 'off' ? c.toUpperCase() : c,
      }),
    );
    if (row === 3) caps.push({ kind: 'delete', row, left: 85, width: 15, label: '' });
  });
  caps.push({ kind: 'layer', row: 4, left: 0, width: 25, label: t.digits ? 'ABC' : '123' });
  caps.push({ kind: 'space', row: 4, left: 25, width: 50, label: 'space' });
  caps.push({ kind: 'return', row: 4, left: 75, width: 25, label: 'OK' });
  return caps;
}

export function typeKey(s: UnifiedState, t: Typing, cap: KeyCap): UnifiedState {
  const put = (next: Typing) => ({ ...s, keyboard: next });
  switch (cap.kind) {
    case 'return': {
      const value = t.text.trim();
      if (t.target === 'recipe-name')
        return { ...s, keyboard: null, recipeName: value || s.recipeName };
      if (t.target === 'recipe-saveas') {
        return {
          ...s,
          keyboard: null,
          recipeName: value || s.recipeName,
          savedRecipe: { ...s.recipe },
          savedFilmId: s.filmId,
        };
      }
      if (t.target === 'film-saveas') {
        return commitNewFilm(s, value, t.switchAfterId);
      }
      return { ...s, keyboard: null };
    }
    case 'delete':
      return t.cursor
        ? put({
            ...t,
            text: t.text.slice(0, t.cursor - 1) + t.text.slice(t.cursor),
            cursor: t.cursor - 1,
          })
        : s;
    case 'shift':
      return put({
        ...t,
        shift: t.shift === 'off' ? 'once' : t.shift === 'once' ? 'locked' : 'off',
      });
    case 'layer':
      return put({ ...t, digits: !t.digits });
    case 'caret-left':
      return put({ ...t, cursor: Math.max(0, t.cursor - 1) });
    case 'caret-right':
      return put({ ...t, cursor: Math.min(t.text.length, t.cursor + 1) });
  }
  const limit = 18;
  const c = cap.kind === 'space' ? ' ' : cap.label;
  if (t.text.length >= limit) return s;
  return put({
    ...t,
    text: t.text.slice(0, t.cursor) + c + t.text.slice(t.cursor),
    cursor: t.cursor + 1,
    shift: t.shift === 'once' ? 'off' : t.shift,
  });
}

function pressKeyboard(s: UnifiedState, t: Typing, key: CameraKey): UnifiedState {
  const caps = keyCaps(t);
  const at = caps[t.focus];
  const center = (c: KeyCap) => c.left + c.width / 2;
  const nearest = (row: number) => {
    let best = -1;
    caps.forEach((c, i) => {
      if (
        c.row === row &&
        (best < 0 || Math.abs(center(c) - center(at)) < Math.abs(center(caps[best]) - center(at)))
      )
        best = i;
    });
    return best;
  };
  const inRow = caps
    .map((c, i) => [c, i] as const)
    .filter(([c]) => c.row === at.row)
    .map(([, i]) => i);
  const k = inRow.indexOf(t.focus);
  switch (key) {
    case 'left':
      return { ...s, keyboard: { ...t, focus: inRow[(k + inRow.length - 1) % inRow.length] } };
    case 'right':
      return { ...s, keyboard: { ...t, focus: inRow[(k + 1) % inRow.length] } };
    case 'up':
      return at.row > 0 ? { ...s, keyboard: { ...t, focus: nearest(at.row - 1) } } : s;
    case 'down':
      return at.row < 4 ? { ...s, keyboard: { ...t, focus: nearest(at.row + 1) } } : s;
    case 'ok':
      return typeKey(s, t, at);
    case 'dial-':
      return typeKey(s, t, caps[0]);
    case 'dial+':
      return typeKey(s, t, caps[1]);
    case 'back':
      return { ...s, keyboard: null };
  }
  return s;
}

export const padText = (v: number, decimals: boolean) =>
  (decimals ? v.toFixed(2) : String(v)).replace('-', '−');

export function padTarget(
  s: UnifiedState,
  pad: Pad,
): {
  name: string;
  value: number;
  min: number;
  max: number;
  step: number;
  decimals: boolean;
  put: (f: Film, v: number) => Film;
} {
  if (pad.cell !== undefined) {
    const labels = pad.tool === 'mix' ? ['R', 'G', 'B'] : ['Cb', 'Cr'];
    const size = MATRIX_SIZE[pad.tool];
    const at = pad.cell;
    return {
      name: `${labels[Math.floor(at / size)]} FROM ${labels[at % size]}`,
      value: matrixCells(s.film, pad.tool)[at],
      min: -4,
      max: 4,
      step: 0.01,
      decimals: true,
      put: (f, v) => putMatrixCell(f, pad.tool, at, v),
    };
  }
  const scale =
    pad.tool === 'chroma'
      ? CHROMA_SCALES[pad.scaleIndex ?? 0]
      : mixScales(s.primary)[pad.scaleIndex ?? 0];
  return {
    name: scale.name,
    value: scale.get(s.film),
    min: scale.min,
    max: scale.max,
    step: scale.step,
    decimals: false,
    put: scale.put,
  };
}

export function openPad(
  s: UnifiedState,
  tool: 'chroma' | 'mix',
  opts: { cell?: number; scaleIndex?: number },
): UnifiedState {
  const pad: Pad = {
    tool,
    cell: opts.cell,
    scaleIndex: opts.scaleIndex,
    text: '',
    fresh: true,
    at: 13,
  };
  const target = padTarget(s, pad);
  return { ...s, pad: { ...pad, text: padText(target.value, target.decimals) } };
}

export function pressPad(
  s: UnifiedState,
  pad: Pad,
  key: CameraKey | { press: PadKey; at: number },
): UnifiedState {
  const target = padTarget(s, pad);
  if (typeof key === 'object' || key === 'ok') {
    const at = typeof key === 'object' ? key.at : pad.at;
    const k =
      typeof key === 'object'
        ? key.press
        : (['7', '8', '9', '4', '5', '6', '1', '2', '3', '±', '0', '.', '⌫', 'OK'] as PadKey[])[
            pad.at
          ];
    if (k === 'OK') {
      const v = padValue(pad.text);
      if (v === undefined) return { ...s, pad: null };
      const snapped =
        Math.round(Math.max(target.min, Math.min(target.max, v)) / target.step) * target.step;
      return { ...s, pad: null, film: target.put(s.film, +snapped.toFixed(2)) };
    }
    return {
      ...s,
      pad: { ...pad, at, text: padType(pad.text, pad.fresh, k, target.decimals), fresh: false },
    };
  }
  switch (key) {
    case 'up':
    case 'down':
    case 'left':
    case 'right':
      return { ...s, pad: { ...pad, at: padStep(pad.at, key) } };
    case 'dial-':
    case 'dial+':
      return {
        ...s,
        pad: { ...pad, at: Math.max(0, Math.min(13, pad.at + (key === 'dial+' ? 1 : -1))) },
      };
    case 'back':
      return { ...s, pad: null };
  }
  return s;
}

/* ---- Vertical focus order inside the scroller ------------------------------------ */

function linearStops(s: UnifiedState): FocusTarget[] {
  const stops: FocusTarget[] = [];
  if (s.mode === 'raw' && !s.collapsed.raw) {
    stops.push({ kind: 'raw', item: 'strip' });
    RAW_OPTIONS.forEach((_, i) => stops.push({ kind: 'raw', item: i }));
    stops.push({ kind: 'raw', item: 'reflect' }, { kind: 'raw', item: 'create' });
  }
  if (!s.collapsed.recipe) {
    stops.push({ kind: 'recipe', item: 'name' }, { kind: 'recipe', item: 'save' });
  }
  stops.push({ kind: 'film-tiles' });
  stops.push({ kind: 'film-header' });
  if (!s.collapsed.film) {
    if (isFilmDirty(s)) {
      stops.push({ kind: 'film-action', action: activeFilmEntry(s).isCard ? 'update' : 'saveas' });
    }
    stops.push({ kind: 'film-tool-header', tool: 'curve' }, { kind: 'film-curve' });
    stops.push(
      { kind: 'film-tool-header', tool: 'chroma' },
      { kind: 'film-chroma', row: 0 },
      { kind: 'film-chroma', row: 1 },
      { kind: 'film-chroma', row: 2 },
      { kind: 'film-chroma', row: 3 },
    );
    stops.push(
      { kind: 'film-tool-header', tool: 'mix' },
      { kind: 'film-mix', row: 'chips' },
      { kind: 'film-mix', row: 0 },
      { kind: 'film-mix', row: 1 },
      { kind: 'film-mix', row: 'numbers' },
    );
    stops.push({ kind: 'film-tool-header', tool: 'luma' }, { kind: 'film-luma' });
  }
  for (const group of SETTING_GROUPS) {
    if (!s.collapsed[group]) {
      for (const setting of RECIPE_SETTINGS) {
        if (setting.group === group) stops.push({ kind: 'setting', id: setting.id });
      }
    }
  }
  return stops;
}

function sameFocus(a: FocusTarget, b: FocusTarget): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

export function enterSection(s: UnifiedState, id: RailId): UnifiedState {
  if (id === 'raw')
    return {
      ...s,
      collapsed: { ...s.collapsed, raw: false },
      focus: { kind: 'raw', item: 'strip' },
      scrollTo: undefined,
    };
  if (id === 'recipe')
    return {
      ...s,
      collapsed: { ...s.collapsed, recipe: false },
      focus: { kind: 'recipe', item: 'name' },
      scrollTo: undefined,
    };
  if (id === 'film') return { ...s, focus: { kind: 'film-tiles' }, scrollTo: undefined };
  if (id === 'curve')
    return {
      ...s,
      collapsed: { ...s.collapsed, film: false },
      focus: { kind: 'film-curve' },
      scrollTo: undefined,
    };
  if (id === 'chroma')
    return {
      ...s,
      collapsed: { ...s.collapsed, film: false },
      focus: { kind: 'film-chroma', row: 0 },
      scrollTo: undefined,
    };
  if (id === 'mix')
    return {
      ...s,
      collapsed: { ...s.collapsed, film: false },
      focus: { kind: 'film-mix', row: 'chips' },
      scrollTo: undefined,
    };
  if (id === 'luma')
    return {
      ...s,
      collapsed: { ...s.collapsed, film: false },
      focus: { kind: 'film-luma' },
      scrollTo: undefined,
    };
  const first = RECIPE_SETTINGS.find((r) => r.group === id);
  return {
    ...s,
    filmOnly: false,
    collapsed: { ...s.collapsed, [id]: false },
    focus: first ? { kind: 'setting', id: first.id } : s.focus,
    scrollTo: undefined,
  };
}

/* ---- Main key handler ------------------------------------------------------------ */

export function press(s: UnifiedState, key: CameraKey): UnifiedState {
  if (s.keyboard) return pressKeyboard(s, s.keyboard, key);
  if (s.pad) return pressPad(s, s.pad, key);

  /* Switch film confirmation modal. */
  if (s.switchModal) {
    const opts = switchModalOptions(s);
    if (key === 'back') return { ...s, switchModal: null };
    if (key === 'up' || key === 'dial-') {
      return {
        ...s,
        switchModal: { ...s.switchModal, focus: Math.max(0, s.switchModal.focus - 1) },
      };
    }
    if (key === 'down' || key === 'dial+') {
      return {
        ...s,
        switchModal: {
          ...s.switchModal,
          focus: Math.min(opts.length - 1, s.switchModal.focus + 1),
        },
      };
    }
    if (key === 'ok') {
      return applySwitchModalChoice(s, opts[s.switchModal.focus]?.id ?? 'cancel');
    }
    return s;
  }

  /* Reflect shooting conditions confirmation modal. */
  if (s.reflectModal) {
    if (key === 'back') return { ...s, reflectModal: null };
    if (key === 'up' || key === 'dial-') {
      return { ...s, reflectModal: { focus: Math.max(0, s.reflectModal.focus - 1) } };
    }
    if (key === 'down' || key === 'dial+') {
      return { ...s, reflectModal: { focus: Math.min(1, s.reflectModal.focus + 1) } };
    }
    if (key === 'ok') {
      return applyReflectModalChoice(s, s.reflectModal.focus === 0);
    }
    return s;
  }

  /* Pickers. */
  if (s.picker) {
    const p = s.picker;
    const count =
      p.kind === 'copy'
        ? BASES.length
        : p.kind === 'open-recipe'
          ? CARD_RECIPES.length
          : p.kind === 'compare'
            ? BASES.length + 1
            : p.kind === 'raw-option'
              ? (RAW_OPTIONS[p.index]?.values.length ?? 0)
              : ((RECIPE_SETTINGS.find((r) => r.id === p.settingId) as ChoiceSetting | undefined)
                  ?.choices.length ?? 0);
    const at = pickerStep(p.at, count, key, 1);
    if (at !== undefined) return { ...s, picker: { ...p, at } };
    if (key === 'back') return { ...s, picker: null };
    if (key === 'ok') {
      if (p.kind === 'copy') return { ...s, picker: null, film: copyGroup(s.film, p.tool, p.at) };
      if (p.kind === 'open-recipe') {
        const chosen = CARD_RECIPES[p.at];
        if (!chosen) return { ...s, picker: null };
        const entry = s.films.find((f) => f.id === chosen.filmId) ?? s.films[0];
        return {
          ...s,
          picker: null,
          slot: chosen.slot,
          recipeName: chosen.name,
          recipeAuthor: chosen.author,
          recipe: { ...chosen.recipe },
          savedRecipe: { ...chosen.recipe },
          savedFilmId: entry.id,
          filmId: entry.id,
          film: cloneFilm(entry.film),
        };
      }
      if (p.kind === 'compare') {
        return {
          ...s,
          picker: null,
          compare: { b: p.at === 0 ? 'opened' : p.at - 1, split: s.compare?.split ?? SPLIT },
        };
      }
      if (p.kind === 'raw-option') {
        const raw = [...s.raw];
        raw[p.index] = p.at;
        return { ...s, picker: null, raw };
      }
      if (p.kind === 'setting-list') {
        return {
          ...s,
          picker: null,
          filmOnly: false,
          recipe: { ...s.recipe, [p.settingId]: p.at },
        };
      }
    }
    return s;
  }

  /* Numbers matrix sheet. */
  if (s.numbers) {
    const n = s.numbers;
    const size = MATRIX_SIZE[n.tool];
    const move = (dr: number, dc: number) => {
      let r = Math.floor(n.cell / size);
      let c = n.cell % size;
      for (let tries = 0; tries < size; tries++) {
        r = Math.max(0, Math.min(size - 1, r + dr));
        c = Math.max(0, Math.min(size - 1, c + dc));
        if (!fixedCell(n.tool, r * size + c))
          return { ...s, numbers: { ...n, cell: r * size + c } };
      }
      return s;
    };
    switch (key) {
      case 'left':
        return move(0, -1);
      case 'right':
        return move(0, 1);
      case 'up':
        return move(-1, 0);
      case 'down':
        return move(1, 0);
      case 'dial-':
      case 'dial+':
        return {
          ...s,
          film: putMatrixCell(
            s.film,
            n.tool,
            n.cell,
            matrixCells(s.film, n.tool)[n.cell] + (key === 'dial+' ? 0.01 : -0.01),
          ),
        };
      case 'ok':
        return fixedCell(n.tool, n.cell) ? s : openPad(s, n.tool, { cell: n.cell });
      case 'back':
        return { ...s, numbers: null };
    }
    return s;
  }

  /* WB SHIFT flyout. */
  if (s.wbShift.open) {
    const clamp = (v: number) => Math.max(-9, Math.min(9, v));
    if (key === 'left')
      return { ...s, recipe: { ...s.recipe, shiftR: clamp(s.recipe.shiftR - 1) } };
    if (key === 'right')
      return { ...s, recipe: { ...s.recipe, shiftR: clamp(s.recipe.shiftR + 1) } };
    if (key === 'up') return { ...s, recipe: { ...s.recipe, shiftB: clamp(s.recipe.shiftB + 1) } };
    if (key === 'down')
      return { ...s, recipe: { ...s.recipe, shiftB: clamp(s.recipe.shiftB - 1) } };
    if (key === 'ok') return { ...s, wbShift: { open: false } };
    if (key === 'back') {
      const [shiftR, shiftB] = s.wbShift.from ?? [s.recipe.shiftR, s.recipe.shiftB];
      return { ...s, recipe: { ...s.recipe, shiftR, shiftB }, wbShift: { open: false } };
    }
    return s;
  }

  /* Global shortcuts: Q = A|B compare, PLAY = RAW toggle, Fn = FILM ONLY or WB SHIFT. */
  if (key === 'q') {
    return { ...s, compare: s.compare ? null : { b: 'opened', split: SPLIT } };
  }
  if (key === 'play') {
    const nextMode = s.mode === 'raw' ? 'live' : 'raw';
    return {
      ...s,
      mode: nextMode,
      focus: nextMode === 'raw' ? { kind: 'raw', item: 'strip' } : { kind: 'film-tiles' },
      inside: false,
      scrollTo: nextMode === 'raw' ? 'raw' : undefined,
    };
  }
  if (key === 'fn') {
    if (s.focus.kind === 'setting' && s.focus.id === 'wb') {
      return {
        ...s,
        filmOnly: false,
        wbShift: { open: true, from: [s.recipe.shiftR, s.recipe.shiftB] },
      };
    }
    if (isInFilmArea(s.focus) || s.filmOnly) {
      return { ...s, filmOnly: !s.filmOnly };
    }
    return s;
  }

  const d = key === 'left' || key === 'down' || key === 'dial-' ? -1 : 1;
  const step = key === 'left' || key === 'dial-' ? -1 : 1;

  /* Inside Tone Curve or Luma Sat interactive graph. */
  if (s.inside) {
    if (key === 'back')
      return s.fullCurve ? { ...s, fullCurve: false } : { ...s, inside: false, bend: false };
    if (key === 'ok')
      return s.focus.kind === 'film-curve'
        ? { ...s, fullCurve: !s.fullCurve }
        : { ...s, inside: false };
    if (s.focus.kind === 'film-curve') {
      if (key === 'left' || key === 'right')
        return { ...s, knot: Math.max(0, Math.min(9, s.knot + d)), bend: false };
      if (key === 'dial-' || key === 'dial+') {
        const tilt = [...curveTilt(s.film, s.curveChannel)];
        tilt[s.knot] = Math.round((tilt[s.knot] + d * 0.1) * 10) / 10;
        return {
          ...s,
          bend: true,
          film: setCurveData(s.film, s.curveChannel, curveKnots(s.film, s.curveChannel), tilt),
        };
      }
      const knots = [...curveKnots(s.film, s.curveChannel)];
      knots[s.knot] = fromDisp(disp(knots[s.knot]) + d / 70);
      return {
        ...s,
        bend: false,
        film: setCurveData(s.film, s.curveChannel, knots, curveTilt(s.film, s.curveChannel)),
      };
    }
    if (s.focus.kind === 'film-luma') {
      if (key === 'left' || key === 'right')
        return { ...s, band: Math.max(0, Math.min(4, s.band + d)) };
      const bands = [...s.film.bands];
      bands[s.band] = Math.max(-100, Math.min(100, bands[s.band] + d * 5));
      return { ...s, film: { ...s.film, bands } };
    }
  }

  /* Quick-scroll rail navigation. */
  if (s.focus.kind === 'rail') {
    const list = visibleTopSections(s);
    const at = list.indexOf(sectionOfFocus(s.focus));
    if (key === 'up' || key === 'dial-') {
      const next = list[Math.max(0, at - 1)];
      return { ...s, focus: { kind: 'rail', id: next }, scrollTo: next };
    }
    if (key === 'down' || key === 'dial+') {
      const next = list[Math.min(list.length - 1, at + 1)];
      return { ...s, focus: { kind: 'rail', id: next }, scrollTo: next };
    }
    if (key === 'right' || key === 'ok') {
      return enterSection(s, s.focus.id);
    }
    return s;
  }

  /* BACK returns to the quick-scroll rail on the current section. */
  if (key === 'back') {
    return {
      ...s,
      focus: { kind: 'rail', id: sectionOfFocus(s.focus) },
      inside: false,
      bend: false,
    };
  }

  /* UP / DOWN moves between vertical focus stops. */
  if (key === 'up' || key === 'down') {
    const stops = linearStops(s);
    let idx = stops.findIndex((t) => sameFocus(t, s.focus));
    if (idx < 0 && s.focus.kind === 'recipe')
      idx = stops.findIndex((t) => t.kind === 'recipe' && t.item === 'save');
    if (idx < 0 && s.focus.kind === 'film-action')
      idx = stops.findIndex((t) => t.kind === 'film-action');
    const nextIdx = Math.max(
      0,
      Math.min(stops.length - 1, (idx < 0 ? 0 : idx) + (key === 'up' ? -1 : 1)),
    );
    const nextFocus = stops[nextIdx];
    const clearFilmOnly = nextFocus.kind === 'setting' ? false : s.filmOnly;
    return { ...s, focus: nextFocus, filmOnly: clearFilmOnly, scrollTo: undefined };
  }

  /* Horizontal / OK interactions per focused item. */
  const f = s.focus;
  if (f.kind === 'raw') {
    if (f.item === 'strip') {
      if (key === 'ok') return { ...s, focus: { kind: 'raw', item: 'create' } };
      return { ...s, crop: Math.max(1, Math.min(RAWS.length, s.crop + step)), created: 0 };
    }
    if (f.item === 'reflect') {
      return key === 'ok' ? requestReflectShootingCond(s) : s;
    }
    if (f.item === 'create') {
      return key === 'ok' ? { ...s, created: s.crop } : s;
    }
    if (typeof f.item === 'number') {
      if (key === 'ok') {
        return { ...s, picker: { kind: 'raw-option', index: f.item, at: s.raw[f.item] ?? 0 } };
      }
      if (key === 'left' || key === 'right' || key === 'dial-' || key === 'dial+') {
        const opt = RAW_OPTIONS[f.item];
        const raw = [...s.raw];
        raw[f.item] = Math.max(0, Math.min(opt.values.length - 1, raw[f.item] + step));
        return { ...s, raw };
      }
      return s;
    }
    return s;
  }

  if (f.kind === 'recipe') {
    if (f.item === 'name') {
      return key === 'ok' ? { ...s, keyboard: typing(s.recipeName, 'recipe-name') } : s;
    }
    const buttons: ('save' | 'saveas' | 'open')[] = ['save', 'saveas', 'open'];
    const bi = buttons.indexOf(f.item);
    if (key === 'left' || key === 'right') {
      const next = buttons[Math.max(0, Math.min(buttons.length - 1, bi + step))];
      return { ...s, focus: { kind: 'recipe', item: next } };
    }
    if (key === 'ok') {
      if (f.item === 'save') return { ...s, savedRecipe: { ...s.recipe }, savedFilmId: s.filmId };
      if (f.item === 'saveas')
        return { ...s, keyboard: typing(nextName(s.recipeName), 'recipe-saveas') };
      if (f.item === 'open') return { ...s, picker: { kind: 'open-recipe', at: 0 } };
    }
    return s;
  }

  if (f.kind === 'film-header') {
    if (key === 'ok' || key === 'left' || key === 'right') {
      return { ...s, collapsed: { ...s.collapsed, film: !s.collapsed.film } };
    }
    return s;
  }

  if (f.kind === 'film-tiles') {
    if (key === 'ok') {
      return { ...s, collapsed: { ...s.collapsed, film: !s.collapsed.film } };
    }
    if (key === 'left' || key === 'right' || key === 'dial-' || key === 'dial+') {
      const idx = s.films.findIndex((entry) => entry.id === s.filmId);
      const nextIdx = Math.max(0, Math.min(s.films.length - 1, idx + step));
      return requestFilmSelect(s, s.films[nextIdx].id);
    }
    return s;
  }

  if (f.kind === 'film-action') {
    const actions: ('update' | 'saveas' | 'reset')[] = activeFilmEntry(s).isCard
      ? ['update', 'saveas', 'reset']
      : ['saveas', 'reset'];
    const ai = Math.max(0, actions.indexOf(f.action));
    if (key === 'left' || key === 'right') {
      return {
        ...s,
        focus: {
          kind: 'film-action',
          action: actions[Math.max(0, Math.min(actions.length - 1, ai + step))],
        },
      };
    }
    if (key === 'ok') {
      if (f.action === 'update') return updateCardFilm(s);
      if (f.action === 'saveas') return openSaveFilmKeyboard(s);
      return resetFilmEdits(s);
    }
    return s;
  }

  if (f.kind === 'film-tool-header') {
    if (key === 'ok') {
      const src = groupSource(s.film, f.tool) ?? 0;
      return { ...s, picker: { kind: 'copy', tool: f.tool, at: src } };
    }
    return s;
  }

  if (f.kind === 'film-curve') {
    if (key === 'ok') return { ...s, inside: true };
    if (key === 'left' || key === 'right') {
      const channels: CurveChannel[] = ['all', 'r', 'g', 'b'];
      const ci = channels.indexOf(s.curveChannel);
      return {
        ...s,
        curveChannel: channels[Math.max(0, Math.min(channels.length - 1, ci + step))],
      };
    }
    return s;
  }

  if (f.kind === 'film-chroma') {
    if (f.row === 3) {
      return key === 'ok' ? { ...s, numbers: { tool: 'chroma', cell: 0 } } : s;
    }
    const scale = CHROMA_SCALES[f.row];
    if (key === 'ok') return openPad(s, 'chroma', { scaleIndex: f.row });
    if (key === 'left' || key === 'right' || key === 'dial-' || key === 'dial+') {
      const nextVal = Math.max(
        scale.min,
        Math.min(scale.max, scale.get(s.film) + step * scale.step),
      );
      return { ...s, film: scale.put(s.film, nextVal) };
    }
    return s;
  }

  if (f.kind === 'film-mix') {
    if (f.row === 'chips') {
      if (key === 'left' || key === 'right' || key === 'dial-' || key === 'dial+') {
        return { ...s, primary: Math.max(0, Math.min(2, s.primary + step)) };
      }
      return s;
    }
    if (f.row === 'numbers') {
      return key === 'ok' ? { ...s, numbers: { tool: 'mix', cell: 1 } } : s;
    }
    const scale = mixScales(s.primary)[f.row];
    if (key === 'ok') return openPad(s, 'mix', { scaleIndex: f.row });
    if (key === 'left' || key === 'right' || key === 'dial-' || key === 'dial+') {
      const nextVal = Math.max(
        scale.min,
        Math.min(scale.max, scale.get(s.film) + step * scale.step),
      );
      return { ...s, film: scale.put(s.film, nextVal) };
    }
    return s;
  }

  if (f.kind === 'film-luma') {
    if (key === 'ok') return { ...s, inside: true };
    return s;
  }

  if (f.kind === 'setting') {
    const setting = RECIPE_SETTINGS.find((r) => r.id === f.id);
    if (!setting) return s;
    if (key === 'ok' && setting.kind === 'choice') {
      return {
        ...s,
        filmOnly: false,
        picker: { kind: 'setting-list', settingId: setting.id, at: s.recipe[setting.id] ?? 0 },
      };
    }
    if (key === 'left' || key === 'right' || key === 'dial-' || key === 'dial+') {
      const count = setting.kind === 'slider' ? setting.steps.length : setting.choices.length;
      const cur = s.recipe[setting.id] ?? 0;
      const nextVal = Math.max(0, Math.min(count - 1, cur + step));
      return { ...s, filmOnly: false, recipe: { ...s.recipe, [setting.id]: nextVal } };
    }
  }

  return s;
}
