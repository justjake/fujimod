import { For, Show, createSignal, onSettled } from 'solid-js';

import { actualSize, loadCalibration, pxPerInch, setActualSize, setPxPerInch } from './screen';
import type { JSX } from '@solidjs/web';

/* Looks: each names how every role is drawn (the column, the rail, rows and
   their focus, chips, panels, the keyboard) as CSS variables on .fm-screen,
   which the materials in materials.ts read. DesignFrame wraps a prototype page
   with the look switches. */

/* The recipe editor's ROUNDED CORNERS and UPPERCASE options, over every
   screen on the page. Round marks (knots, swatches, dots) stay round. */
/* Materials: each look names how every role is drawn (the column, the rail,
   rows and their focus, chips, panels, the keyboard), not just two colors. */
const LOOKS = {
  /* Drawn from its two dials: glassVars. */
  glass: '',
  contrast: `
    --m-column: rgb(0 0 0 / 0.88); --m-column-edge: inset -2px 0 0 #fff;
    --m-popup: #000; --m-popup-edge: inset 0 0 0 2px #fff; --m-cursor: rgb(255 255 255 / 0.2); --m-cursor-edge: inset 0 0 0 3px #ffd60a; --m-separator: rgb(255 255 255 / 0.4); --m-check: #fff;
    --m-panel: #000;
    --m-chip: #000; --m-chip-edge: inset 0 0 0 2px #fff;
    --m-track: rgb(255 255 255 / 0.1); --m-track-edge: inset 0 0 0 1px rgb(255 255 255 / 0.45);
    --m-focus: rgb(255 255 255 / 0.2); --m-focus-edge: inset 0 0 0 3px #ffd60a;
    --m-divider: inset 0 2px 0 rgb(255 255 255 / 0.55);
    --m-accent: #b06cff; --m-confirm: #1f7cff;
    --m-title: #000; --m-text-area: #000; --m-tray: #000;
    --m-well: #000; --m-well-edge: inset 0 0 0 1px rgb(255 255 255 / 0.4); --m-mark: #ffd60a;
    --m-key: #3a3a3c; --m-key-special: #1c1c1e; --m-key-edge: inset 0 0 0 1px rgb(255 255 255 / 0.5); --m-key-focus: inset 0 0 0 4px #ffd60a;`,
  /* After the X-series menus: an opaque black column, gray plates with a lit
     top edge and a dark bottom edge, engraved dividers, recessed wells for
     scales, and focus as a brighter raised plate with a fine light rim. */
  fujimod: `
    --m-popup: #1e1e20; --m-popup-edge: inset 0 0 0 1px #3e3e42; --m-cursor: #3a3a3c; --m-cursor-edge: inset 0 0 0 0 transparent; --m-separator: #323235; --m-check: #2a7fff;
    --m-column: linear-gradient(#111 0%, #070707 100%); --m-column-edge: inset -1px 0 0 #000, inset -2px 0 0 rgb(255 255 255 / 0.1);
    --m-panel: linear-gradient(#1b1b1b, #0e0e0e);
    --m-chip: linear-gradient(#3a3a3a, #262626); --m-chip-edge: inset 0 1px 0 rgb(255 255 255 / 0.2), inset 0 -1px 0 rgb(0 0 0 / 0.7), 0 1px 2px rgb(0 0 0 / 0.6);
    --m-track: linear-gradient(#262626, #1a1a1a); --m-track-edge: inset 0 1px 0 rgb(255 255 255 / 0.12), inset 0 -1px 0 rgb(0 0 0 / 0.8), 0 1px 0 rgb(0 0 0 / 0.9);
    --m-focus: linear-gradient(#5c5c5e, #3d3d3f); --m-focus-edge: inset 0 1px 0 rgb(255 255 255 / 0.35), inset 0 -1px 0 rgb(0 0 0 / 0.6), 0 0 0 1px rgb(255 255 255 / 0.55), 0 2px 4px rgb(0 0 0 / 0.7);
    --m-divider: inset 0 1px 0 #000, inset 0 2px 0 rgb(255 255 255 / 0.09);
    --m-well: linear-gradient(#060606, #121212); --m-well-edge: inset 0 2px 3px rgb(0 0 0 / 0.9), inset 0 -1px 0 rgb(255 255 255 / 0.1);
    --m-mark: #e5333a;
    --m-accent: #d4282f; --m-confirm: linear-gradient(#d83a40, #a91d23);
    --m-title: linear-gradient(#262626, #121212); --m-text-area: rgb(0 0 0 / 0.82); --m-tray: linear-gradient(#141414, #050505);
    --m-key: linear-gradient(#4a4a4a, #333); --m-key-special: linear-gradient(#2c2c2c, #1e1e1e);
    --m-key-edge: inset 0 1px 0 rgb(255 255 255 / 0.22), inset 0 -1px 0 rgb(0 0 0 / 0.7), 0 1px 2px rgb(0 0 0 / 0.8);
    --m-key-focus: inset 0 1px 0 rgb(255 255 255 / 0.4), 0 0 0 2px #fff;`,
};
type Look = keyof typeof LOOKS;
const LOOK_NAMES: [Look, string][] = [['glass', 'Glass'], ['contrast', 'High contrast'], ['fujimod', 'Fujimod menu']];

/* Glass has three dials: OPACITY (how much of the photo shows through),
   CONTRAST (how bright edges, focus and dividers are) and LIGHTNESS (how light
   the glass itself is, from smoked to pale gray). */
function glassVars(opacity: number, contrast: number, lightness: number): string {
  const mix = (a: number, b: number, t: number) => +(a + (b - a) * t).toFixed(3);
  /* A surface's gray: its dark shade at LIGHTNESS 0, a light gray at 100. */
  const gray = (dark: number, blue = 0) => {
    const v = Math.round(mix(dark, 150, lightness));
    return `${v} ${v} ${v + blue}`;
  };
  /* A surface's alpha: its own at OPACITY 0, fully opaque at 100; eased so
     the default 35 keeps the look it was tuned at. */
  const fill = (a: number) => +(a + (1 - a) * opacity ** 1.5).toFixed(3);
  const edge = (a: number) => mix(a, Math.min(1, a * 2.2), contrast);
  return `
    --m-column: rgb(${gray(mix(22, 12, opacity), 2)} / ${fill(0.5)});
    --m-column-edge: inset -1px 0 0 rgb(255 255 255 / ${edge(0.2)});
    --m-popup: rgb(${gray(mix(22, 12, opacity), 2)} / ${fill(0.5)}); --m-popup-edge: inset 0 0 0 1px rgb(255 255 255 / ${edge(0.16)});
    --m-separator: rgb(255 255 255 / ${edge(0.12)}); --m-check: #fff;
    --m-cursor: rgb(255 255 255 / 0.24); --m-cursor-edge: inset 0 0 0 2px rgb(255 255 255 / 0.85);
    --m-panel: rgb(${gray(26, 2)} / ${fill(0.84)});
    --m-chip: rgb(${gray(20, 2)} / ${fill(0.52)});
    --m-chip-edge: inset 0 1px 0 rgb(255 255 255 / ${edge(0.25)}), inset 0 0 0 1px rgb(255 255 255 / ${edge(0.09)});
    --m-track: rgb(255 255 255 / ${mix(0.05, 0.1, contrast)}); --m-track-edge: inset 0 0 0 1px rgb(255 255 255 / ${mix(0, 0.12, contrast)});
    --m-focus: rgb(255 255 255 / ${mix(0.15, 0.26, contrast)});
    --m-focus-edge: inset 0 1px 0 rgb(255 255 255 / ${edge(0.3)}), inset 0 0 0 ${contrast > 0.5 ? 2 : 1}px rgb(255 255 255 / ${edge(0.12) + contrast * 0.3});
    --m-divider: inset 0 1px 0 rgb(255 255 255 / ${edge(0.14)});
    --m-accent: ${contrast > 0.5 ? '#8f4fe0' : '#7d3fc2'}; --m-confirm: #0a6bdc;
    --m-title: rgb(${gray(0)} / ${fill(0.55)}); --m-text-area: rgb(${gray(20, 2)} / ${fill(0.42)}); --m-tray: rgb(${gray(26, 2)} / ${fill(0.76)});
    --m-key: rgb(255 255 255 / ${mix(0.28, 0.36, contrast)}); --m-key-special: rgb(255 255 255 / ${mix(0.1, 0.17, contrast)});
    --m-key-edge: inset 0 1px 0 rgb(255 255 255 / ${mix(0, 0.25, contrast)}); --m-key-focus: inset 0 0 0 3px #fff;
    --m-well: rgb(0 0 0 / ${mix(0.18, 0.34, opacity)}); --m-well-edge: inset 0 1px 0 rgb(0 0 0 / 0.3); --m-mark: #ff9f0a;`;
}
const GLASS_DEFAULT = { opacity: 0.35, contrast: 0.6, lightness: 0 };

const STYLE_OPTIONS = `
${(Object.keys(LOOKS) as Look[]).filter((k) => k !== 'glass').map((k) => `.fm-look-${k} .fm-screen {${LOOKS[k]} }`).join('\n')}
.fm-look-fujimod .fm-screen *:not(.rounded-full):not(.fm-round) { border-radius: 0 !important; }
.fm-look-fujimod .fm-screen rect { rx: 0; ry: 0; }
.fm-square .fm-screen *:not(.rounded-full):not(.fm-round) { border-radius: 0 !important; }
.fm-square .fm-screen rect { rx: 0; ry: 0; }
.fm-upper .fm-screen { text-transform: uppercase; }
`;

function Toggle(props: { label: string; on: boolean; set: (on: boolean) => void }) {
  return (
    <button
      type="button"
      class="flex h-[44px] items-center gap-[10px] rounded-[12px] bg-[#2c2c2e] pr-[14px] pl-[8px] text-[16px] font-semibold text-white"
      onClick={() => props.set(!props.on)}
    >
      <span class="relative h-[28px] w-[46px] rounded-full transition-colors" style={{ background: props.on ? '#34c759' : '#48484a' }}>
        <span class="absolute top-[2px] size-[24px] rounded-full bg-white transition-[left]" style={{ left: props.on ? '20px' : '2px' }} />
      </span>
      {props.label}
    </button>
  );
}

/** A prototype page: its title, the look switches, then its screens. Every
 * .fm-screen inside takes the chosen look. */
export function DesignFrame(props: { title: string; children: JSX.Element }) {
  const [rounded, setRounded] = createSignal(true);
  const [upper, setUpper] = createSignal(false);
  const [look, setLook] = createSignal<Look>('glass');
  const [opacity, setOpacity] = createSignal(GLASS_DEFAULT.opacity);
  const [contrast, setContrast] = createSignal(GLASS_DEFAULT.contrast);
  const [lightness, setLightness] = createSignal(GLASS_DEFAULT.lightness);
  onSettled(loadCalibration);
  return (
    <div
      class={`design-frame touch-manipulation bg-[#111] px-[12px] py-[16px] font-sans text-[15px] text-[#ddd] ${rounded() ? '' : 'fm-square'} ${upper() ? 'fm-upper' : ''} fm-look-${look()}`}
    >
      <style>{STYLE_OPTIONS}</style>
      <style>{`.fm-look-glass .fm-screen {${glassVars(opacity(), contrast(), lightness())} }`}</style>
      <h1 class="mb-[12px] text-[22px] font-semibold">{props.title}</h1>
      <div class="mb-[14px] flex flex-wrap gap-[8px]">
        <Toggle label="Rounded corners" on={rounded()} set={setRounded} />
        <Toggle label="Uppercase" on={upper()} set={setUpper} />
        <For each={LOOK_NAMES}>
          {([value, label]) => (
            <button
              type="button"
              class="h-[44px] rounded-[12px] px-[14px] text-[16px] font-semibold"
              style={look() === value ? { background: '#fff', color: '#000' } : { background: '#2c2c2e', color: '#fff' }}
              onClick={() => {
                setLook(value);
                if (value === 'fujimod') setUpper(true);
              }}
            >
              {label}
            </button>
          )}
        </For>
      </div>
      <div class="mb-[14px] flex flex-col gap-[10px]">
        <Toggle label="Actual size" on={actualSize()} set={setActualSize} />
        <Show when={actualSize()}>
          <label class="flex h-[44px] max-w-[720px] items-center gap-[12px] text-[16px] font-semibold text-white">
            <span class="w-[84px]">Calibrate</span>
            <input
              type="range"
              min="80"
              max="220"
              value={pxPerInch()}
              class="h-[44px] grow accent-white"
              onInput={(e) => setPxPerInch(Number(e.currentTarget.value))}
            />
          </label>
          {/* A credit card, upright: 54.0 by 85.6 mm. Hold one against this outline. */}
          <div
            class="flex items-center justify-center rounded-[8px] border-2 border-dashed border-white/60 text-[14px] text-white/70"
            style={{ width: `${(53.98 / 25.4) * pxPerInch()}px`, height: `${(85.6 / 25.4) * pxPerInch()}px` }}
          >
            Credit card
          </div>
        </Show>
      </div>
      <Show when={look() === 'glass'}>
        <div class="mb-[14px] flex max-w-[720px] flex-col gap-[6px]">
          <For each={[['Opacity', opacity, setOpacity], ['Contrast', contrast, setContrast], ['Lightness', lightness, setLightness]] as [string, () => number, (v: number) => void][]}>
            {([label, get, set]) => (
              <label class="flex h-[44px] items-center gap-[12px] text-[16px] font-semibold text-white">
                <span class="w-[84px]">{label}</span>
                <input type="range" min="0" max="100" value={Math.round(get() * 100)} class="h-[44px] grow accent-white" onInput={(e) => set(Number(e.currentTarget.value) / 100)} />
                <span class="w-[40px] text-right tabular-nums">{Math.round(get() * 100)}</span>
              </label>
            )}
          </For>
        </div>
      </Show>
      {props.children}
    </div>
  );
}
