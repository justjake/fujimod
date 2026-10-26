import type { Camera } from './cameras';

/**
 * Everything the wizard's illustration can show, as numbers. Timelines (steps.ts) move
 * these values with anime.js and `draw` renders them, so each step starts from where
 * the previous one stopped and any moment can be reproduced by seeking.
 */
export interface Rig {
  /** View box: centre and width in millimetres (height follows the stage's aspect). */
  cx: number;
  cy: number;
  zoom: number;
  /** +1 shows the top plate above the back, -1 the base below it, 0 the back alone. */
  tilt: number;
  laptop: number;
  cable: number;
  plug: number;
  /** Power lever: 0 ON, 1 OFF. */
  lever: number;
  /** Power arrow: drawn fraction, direction (1: ON to OFF, -1: OFF to ON), opacity. */
  arrow: number;
  arrowDir: number;
  arrowAlpha: number;
  holdUp: number;
  holdShutter: number;
  /** Ever-increasing clock for pulses and data flow. */
  phase: number;
  lcd: number;
  led: number;
  latch: number;
  door: number;
  battery: number;
  data: number;
  check: number;
}

export function initialRig(camera: Camera): Rig {
  const a = camera.art;
  return {
    cx: a.width / 2 + 40,
    cy: a.height / 2 - 10,
    zoom: a.width + 120,
    tilt: 0.7,
    laptop: 0,
    cable: 0,
    plug: 0,
    lever: 0,
    arrow: 0,
    arrowDir: 1,
    arrowAlpha: 0,
    holdUp: 0,
    holdShutter: 0,
    phase: 0,
    lcd: 1,
    led: 0,
    latch: 0,
    door: 0,
    battery: 0,
    data: 0,
    check: 0,
  };
}

const NS = 'http://www.w3.org/2000/svg';
const DEPTH_Y = 0.72,
  DEPTH_X = 0.42;
function el<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | number> = {},
  parent?: Element,
): SVGElementTagNameMap[K] {
  const e = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  parent?.append(e);
  return e;
}
const set = (e: Element, attrs: Record<string, string | number>) => {
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
};
const clamp = (v: number, lo = 0, hi = 1) => Math.max(lo, Math.min(hi, v));
const mix = (a: number, b: number, t: number) => a + (b - a) * t;
const rad = (deg: number) => (deg * Math.PI) / 180;

/** Builds one camera's drawing into `svg` and returns its renderer. */
export function buildScene(svg: SVGSVGElement, camera: Camera) {
  svg.replaceChildren();
  const a = camera.art,
    W = a.width,
    H = a.height,
    D = a.depth;
  const defs = el('defs', {}, svg);
  const glow = el(
    'filter',
    { id: 'glow', x: '-50%', y: '-50%', width: '200%', height: '200%' },
    defs,
  );
  el('feGaussianBlur', { stdDeviation: 1.2 }, glow);
  const grad = el('linearGradient', { id: 'lcd-on', x1: 0, y1: 0, x2: 1, y2: 1 }, defs);
  el('stop', { offset: 0, 'stop-color': '#3d5a73' }, grad);
  el('stop', { offset: 0.55, 'stop-color': '#8a7a5c' }, grad);
  el('stop', { offset: 1, 'stop-color': '#2f3b2c' }, grad);

  // ---- laptop and cable (behind the camera) ----
  const laptop = el('g', { class: 'c-laptop' }, svg);
  const LX = W + 46,
    LY = H * 0.12;
  el('rect', { x: LX, y: LY, width: 74, height: 46, rx: 3, class: 'c-lid' }, laptop);
  el('rect', { x: LX + 4, y: LY + 4, width: 66, height: 38, rx: 1, class: 'c-screen' }, laptop);
  el('path', { d: `M${LX - 8} ${LY + 50} h90 l-6 5 h-78 z`, class: 'c-base' }, laptop);
  const lapPort = { x: LX - 6, y: LY + 52 };
  const portY = H * a.usb.y;
  const cablePath = `M${lapPort.x} ${lapPort.y} C ${lapPort.x - 30} ${lapPort.y + 10}, ${W + 34} ${portY}, ${W + 3} ${portY}`;
  const cable = el('path', { d: cablePath, class: 'c-cable', pathLength: 1 }, svg);
  const plug = el('rect', { width: 7, height: 4, rx: 1, class: 'c-plug' }, svg);

  // ---- top plate (oblique, above the back) and base (below it) ----
  const top = el('g', {}, svg);
  const topFace = el('path', { class: 'c-face c-top' }, top);
  const topParts = el('g', {}, top);
  const lensT = a.lens;
  el(
    'rect',
    {
      x: lensT.u - lensT.w / 2,
      y: D - 1,
      width: lensT.w,
      height: lensT.length + 1,
      rx: 3,
      class: 'c-lens',
    },
    topParts,
  );
  el(
    'rect',
    {
      x: lensT.u - lensT.w / 2 + 3,
      y: D + lensT.length - 3,
      width: lensT.w - 6,
      height: 3,
      rx: 1,
      class: 'c-lens-ring',
    },
    topParts,
  );
  el(
    'rect',
    {
      x: a.top.shoe.u - a.top.shoe.w / 2,
      y: 8,
      width: a.top.shoe.w,
      height: 18,
      rx: 1.5,
      class: 'c-shoe',
    },
    topParts,
  );
  if (a.top.flash)
    el(
      'rect',
      {
        x: a.top.flash.u - a.top.flash.w / 2,
        y: 10,
        width: a.top.flash.w,
        height: 22,
        rx: 2,
        class: 'c-shoe',
      },
      topParts,
    );
  for (const d of a.top.dials) {
    el('circle', { cx: d.u, cy: d.v, r: d.r, class: 'c-dial' }, topParts);
    if (d.label) {
      const t = el(
        'text',
        { x: 0, y: 1.1, class: 'c-dial-label', transform: `translate(${d.u} ${d.v}) scale(1 -1)` },
        topParts,
      );
      t.textContent = d.label;
    }
  }
  const sh = a.top.shutter,
    lv = a.top.lever;
  const leverRing = el(
    'circle',
    { cx: sh.u, cy: sh.v, r: lv.r - 2, class: 'c-lever-ring' },
    topParts,
  );
  void leverRing;
  const leverTab = el('path', { class: 'c-lever-tab' }, topParts);
  for (const [label, angle] of [
    ['ON', lv.on],
    ['OFF', lv.off],
  ] as const) {
    const p = polar(sh.u, sh.v, lv.r + 5.5, angle);
    const t = el(
      'text',
      { x: 0, y: 1, class: 'c-lever-label', transform: `translate(${p.x} ${p.y}) scale(1 -1)` },
      topParts,
    );
    t.textContent = label;
  }
  const shutterKey = el(
    'circle',
    { cx: sh.u, cy: sh.v, r: sh.r, class: 'c-key c-shutter' },
    topParts,
  );
  const arrowPath = el('path', { class: 'c-arrow', pathLength: 1 }, topParts);
  const arrowHead = el('path', { d: 'M0 -2.4 L4.2 0 L0 2.4 z', class: 'c-arrow-head' }, topParts);

  const base = el('g', {}, svg);
  const baseFace = el('path', { class: 'c-face c-bottom' }, base);
  const baseParts = el('g', {}, base);
  el(
    'rect',
    {
      x: lensT.u - lensT.w / 2,
      y: D - 1,
      width: lensT.w,
      height: lensT.length + 1,
      rx: 3,
      class: 'c-lens',
    },
    baseParts,
  );
  const bt = a.bottom.battery;
  el(
    'circle',
    { cx: a.bottom.tripod.u, cy: a.bottom.tripod.v, r: 3, class: 'c-tripod' },
    baseParts,
  );
  const latch = el('rect', { width: 6, height: 2.4, rx: 1, class: 'c-latch' }, baseParts);
  el(
    'rect',
    {
      x: bt.u - bt.w / 2 + 1,
      y: bt.v + 1,
      width: bt.w - 2,
      height: bt.d - 2,
      rx: 1.5,
      class: 'c-cavity',
    },
    baseParts,
  );
  const batteryCell = el(
    'rect',
    {
      x: bt.u - bt.w / 2 + 2,
      y: bt.v + 2,
      width: bt.w - 4,
      height: bt.d - 4,
      rx: 1.5,
      class: 'c-battery',
    },
    baseParts,
  );
  const batteryLabel = el(
    'text',
    { x: bt.u, y: bt.v + bt.d / 2 + 1.5, class: 'c-battery-label' },
    baseParts,
  );
  batteryLabel.textContent = 'NP-95';
  const door = el('path', { class: 'c-door' }, baseParts);

  // ---- back of the camera ----
  const rear = el('g', {}, svg);
  el('rect', { x: 0, y: 0, width: W, height: H, rx: 4, class: 'c-face c-rear' }, rear);
  if (a.rear.grip)
    el(
      'rect',
      { x: a.rear.grip.x, y: 2, width: a.rear.grip.w - 2, height: H - 4, rx: 3, class: 'c-grip' },
      rear,
    );
  const r = a.rear;
  if (r.evf)
    el(
      'rect',
      { x: r.evf.x, y: r.evf.y, width: r.evf.w, height: r.evf.h, rx: 2, class: 'c-evf' },
      rear,
    );
  const lcd = el(
    'rect',
    { x: r.lcd.x, y: r.lcd.y, width: r.lcd.w, height: r.lcd.h, rx: 1.5, class: 'c-lcd' },
    rear,
  );
  const lcdImage = el(
    'rect',
    {
      x: r.lcd.x + 1.5,
      y: r.lcd.y + 1.5,
      width: r.lcd.w - 3,
      height: r.lcd.h - 3,
      fill: 'url(#lcd-on)',
    },
    rear,
  );
  void lcd;
  for (const k of r.keys) {
    el('rect', { x: k.x, y: k.y, width: k.w, height: k.h, rx: 1.2, class: 'c-key' }, rear);
    if (k.label) {
      const t = el(
        'text',
        { x: k.x + k.w / 2, y: k.y + k.h / 2 + 1.1, class: 'c-key-label' },
        rear,
      );
      t.textContent = k.label;
    }
  }
  const dp = r.dpad;
  el('circle', { cx: dp.x, cy: dp.y, r: dp.r, class: 'c-dpad' }, rear);
  el('circle', { cx: dp.x, cy: dp.y, r: dp.r * 0.42, class: 'c-key' }, rear);
  const upKey = el(
    'path',
    { d: arc(dp.x, dp.y, dp.r - 0.6, dp.r * 0.5, -135, -45), class: 'c-key c-up' },
    rear,
  );
  for (const [s, e] of [
    [-45, 45],
    [45, 135],
    [135, 225],
  ])
    el('path', { d: arc(dp.x, dp.y, dp.r - 0.6, dp.r * 0.5, s, e), class: 'c-key c-seg' }, rear);
  const upLabel = el('text', { x: dp.x, y: dp.y - dp.r * 0.62, class: 'c-key-label' }, rear);
  upLabel.textContent = '▲';
  const led = el('circle', { cx: r.led.x, cy: r.led.y, r: 1.3, class: 'c-led' }, rear);
  el('rect', { x: W - 1.5, y: portY - 3, width: 2.5, height: 6, rx: 0.8, class: 'c-port' }, rear);

  // ---- callouts in screen space ----
  const callouts = el('g', {}, svg);
  const pulseUp = el('circle', { class: 'c-pulse' }, callouts);
  const pulseShutter = el('circle', { class: 'c-pulse' }, callouts);
  const labelUp = el('g', { class: 'c-callout' }, callouts);
  el('rect', { x: -15, y: -5, width: 30, height: 8.5, rx: 4.25 }, labelUp);
  el('text', { x: 0, y: 1.6 }, labelUp).textContent = 'Hold UP';
  const labelShutter = el('g', { class: 'c-callout' }, callouts);
  el('rect', { x: -27, y: -5, width: 54, height: 8.5, rx: 4.25 }, labelShutter);
  el('text', { x: 0, y: 1.6 }, labelShutter).textContent = 'Hold shutter fully';
  const dots = [0, 1, 2].map(() => el('circle', { r: 1.4, class: 'c-data' }, callouts));
  const check = el('g', { class: 'c-check' }, callouts);
  el('circle', { r: 7 }, check);
  el('path', { d: 'M-3.2 0.2 L-0.8 2.6 L3.6 -2.4' }, check);

  function polar(cx: number, cy: number, r: number, deg: number) {
    return { x: cx + r * Math.cos(rad(deg)), y: cy + r * Math.sin(rad(deg)) };
  }

  return function draw(s: Rig, aspect: number) {
    const w = s.zoom,
      h = w / aspect;
    svg.setAttribute('viewBox', `${s.cx - w / 2} ${s.cy - h / 2} ${w} ${h}`);
    // oblique faces: top shows for tilt > 0, base for tilt < 0
    const up = clamp(s.tilt, 0, 1),
      down = clamp(-s.tilt, 0, 1);
    const tdx = DEPTH_X * up,
      tdy = DEPTH_Y * up,
      bdx = DEPTH_X * down,
      bdy = DEPTH_Y * down;
    top.setAttribute('transform', `matrix(1 0 ${tdx} ${-tdy} 0 0)`);
    top.style.opacity = String(clamp(up * 4));
    set(topFace, { d: `M0 0 H${W} L${W} ${D} H0 Z` });
    base.setAttribute('transform', `matrix(1 0 ${bdx} ${bdy} 0 ${H})`);
    base.style.opacity = String(clamp(down * 4));
    set(baseFace, { d: `M0 0 H${W} L${W} ${D} H0 Z` });
    const toTop = (u: number, v: number) => ({ x: u + v * tdx, y: -v * tdy });
    const toRear = (x: number, y: number) => ({ x, y });

    // power lever and its arrow
    const angle = mix(lv.on, lv.off, s.lever);
    const t0 = polar(sh.u, sh.v, lv.r - 2, angle - 14),
      t1 = polar(sh.u, sh.v, lv.r + 2.6, angle - 9),
      t2 = polar(sh.u, sh.v, lv.r + 2.6, angle + 9),
      t3 = polar(sh.u, sh.v, lv.r - 2, angle + 14);
    set(leverTab, { d: `M${t0.x} ${t0.y} L${t1.x} ${t1.y} L${t2.x} ${t2.y} L${t3.x} ${t3.y} Z` });
    const from = s.arrowDir > 0 ? lv.on : lv.off,
      to = s.arrowDir > 0 ? lv.off : lv.on;
    const ar = lv.r + 10;
    const p0 = polar(sh.u, sh.v, ar, from),
      p1 = polar(sh.u, sh.v, ar, to);
    set(arrowPath, {
      d: `M${p0.x} ${p0.y} A${ar} ${ar} 0 0 ${to > from ? 1 : 0} ${p1.x} ${p1.y}`,
      'stroke-dasharray': `${clamp(s.arrow)} 1`,
      opacity: s.arrowAlpha,
    });
    const tip = mix(from, to, clamp(s.arrow));
    const tp = polar(sh.u, sh.v, ar, tip);
    const heading = tip + (to > from ? 90 : -90);
    set(arrowHead, {
      transform: `translate(${tp.x} ${tp.y}) rotate(${heading})`,
      opacity: s.arrowAlpha * (s.arrow > 0.02 ? 1 : 0),
    });
    shutterKey.classList.toggle('c-held', s.holdShutter > 0.5);
    upKey.classList.toggle('c-held', s.holdUp > 0.5);

    // battery door: slides the latch, then swings open; the battery drops out
    set(latch, { x: bt.u - 3 + s.latch * 4, y: bt.v + bt.d + 1.5 });
    const open = clamp(s.door);
    const hinge = bt.v,
      far = bt.v + bt.d * Math.cos(open * Math.PI * 0.95);
    set(door, {
      d: `M${bt.u - bt.w / 2} ${hinge} H${bt.u + bt.w / 2} V${far} H${bt.u - bt.w / 2} Z`,
      opacity: open > 0.5 ? 0.85 : 1,
    });
    // the battery slides out past the front of the base, ending beside the camera
    const drop = clamp(s.battery) * (bt.d * 1.15);
    const shown = open > 0.55 || s.battery > 0.01 ? 1 : 0;
    set(batteryCell, { transform: `translate(${clamp(s.battery) * 6} ${drop})`, opacity: shown });
    set(batteryLabel, { transform: `translate(${clamp(s.battery) * 6} ${drop})`, opacity: shown });

    // screen and status light
    set(lcdImage, { opacity: clamp(s.lcd) });
    set(led, { class: 'c-led' + (s.led > 0.5 ? ' c-on' : '') });

    // laptop, cable and plug
    laptop.style.opacity = String(clamp(s.laptop));
    set(cable, { 'stroke-dasharray': `${clamp(s.cable)} 1`, opacity: clamp(s.laptop) });
    const end = cable.getPointAtLength(cable.getTotalLength() * clamp(s.cable));
    const seat = clamp(s.plug);
    set(plug, {
      x: end.x - 3.5 + 5 * (1 - seat) * (s.cable > 0.99 ? 1 : 0),
      y: end.y - 2,
      opacity: s.cable > 0.05 ? clamp(s.laptop) : 0,
    });

    // callouts follow the parts they point at
    const upAt = toRear(dp.x, dp.y - dp.r * 0.72),
      shAt = toTop(sh.u, sh.v);
    const ring = (s.phase % 1) * 1;
    for (const [c, at, k] of [
      [pulseUp, upAt, s.holdUp],
      [pulseShutter, shAt, s.holdShutter],
    ] as const)
      set(c, { cx: at.x, cy: at.y, r: 3 + ring * 7, opacity: clamp(k) * (1 - ring) });
    set(labelUp, {
      transform: `translate(${upAt.x - 22} ${upAt.y + 2})`,
      opacity: clamp(s.holdUp),
    });
    set(labelShutter, {
      transform: `translate(${shAt.x - 40} ${shAt.y - 4})`,
      opacity: clamp(s.holdShutter),
    });

    // data moving along the cable (debug step)
    const L = cable.getTotalLength();
    dots.forEach((d, i) => {
      const f = (s.phase * 0.8 + i / 3) % 1;
      const p = cable.getPointAtLength(L * f);
      set(d, { cx: p.x, cy: p.y, opacity: clamp(s.data) * Math.sin(f * Math.PI) });
    });
    set(check, {
      transform: `translate(${W / 2} ${H / 2}) scale(${0.6 + 0.4 * clamp(s.check)})`,
      opacity: clamp(s.check),
    });
  };
}

function arc(cx: number, cy: number, r0: number, r1: number, a0: number, a1: number) {
  const p = (r: number, a: number) => `${cx + r * Math.cos(rad(a))} ${cy + r * Math.sin(rad(a))}`;
  return `M${p(r0, a0)} A${r0} ${r0} 0 0 1 ${p(r0, a1)} L${p(r1, a1)} A${r1} ${r1} 0 0 0 ${p(r1, a0)} Z`;
}
