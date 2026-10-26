import { createTimeline, type Timeline } from 'animejs';
import type { Camera } from './cameras';
import type { Rig } from './scene';

export type StepId = 'enter' | 'connect' | 'debug' | 'exit';
export interface Step {
  id: StepId;
  title: string;
  /** Captions in order; the timeline switches between them. */
  beats: string[];
  loop: boolean;
}
export const steps: Step[] = [
  {
    id: 'enter',
    title: 'Start in service mode',
    loop: true,
    beats: [
      'Connect the camera to this computer with a USB cable.',
      'Turn the camera off.',
      'Hold UP, and press the shutter button all the way down. Keep holding both.',
      'Still holding them, turn the camera on.',
      'Let go. The screen stays dark and the light turns green: the camera is in service mode.',
    ],
  },
  {
    id: 'connect',
    title: 'Connect from this page',
    loop: false,
    beats: ['Leave the camera on and connected. Then connect to it from this page.'],
  },
  {
    id: 'debug',
    title: 'Turn on debug mode',
    loop: false,
    beats: ['The page turns on debug mode over the cable.'],
  },
  {
    id: 'exit',
    title: 'Leave service mode',
    loop: true,
    beats: [
      'Keep the cable plugged in. Turn the camera over and slide the battery door latch.',
      'Open the door and take the battery out.',
      'Now unplug the USB cable.',
      'Turn the switch to OFF, put the battery back and close the door.',
      'Turn the camera on without holding any buttons. It starts normally.',
    ],
  },
];

const ease = 'inOutCubic';
/**
 * The timeline for one step, starting from the rig's current values so the picture
 * moves on from wherever the previous step left it. Caption i starts at label `b<i>`;
 * `beatAt` reads the caption due at any time, so seeking keeps the caption right.
 */
export function stepTimeline(step: StepId, camera: Camera, rig: Rig): Timeline {
  const a = camera.art,
    W = a.width,
    H = a.height,
    D = a.depth,
    sh = a.top.shutter,
    dp = a.rear.dpad;
  const loop = steps.find((s) => s.id === step)!.loop;
  const tl = createTimeline({ loop, loopDelay: 1800, defaults: { ease } });
  // views, as centre + width in millimetres
  const overview = { cx: W / 2 + 52, cy: H / 2 - 8, zoom: W + 150 };
  const body = { cx: W / 2 + 4, cy: H / 2 - D * 0.42, zoom: W + 80 };
  // the shutter, as drawn on the top plate at full tilt (scene.ts DEPTH_X/DEPTH_Y)
  const topPlate = { cx: sh.u + sh.v * 0.42 - 4, cy: -sh.v * 0.72 + 1, zoom: 78 };
  const holdView = {
    cx: (sh.u + dp.x) / 2 - 6,
    cy: (dp.y - sh.v * 0.72 * 0.85) / 2,
    zoom: W * 0.95,
  };
  const bt = a.bottom.battery;
  const baseView = { cx: bt.u - 10, cy: H + D * 0.45, zoom: W * 1.05 };
  const at = (i: number, t: number | string) => {
    tl.call(() => {}, t); // holds the gap before the caption
    tl.label(`b${i}`, tl.iterationDuration);
  };

  if (step === 'enter') {
    tl.add(rig, {
      lever: 0,
      lcd: 1,
      led: 0,
      cable: 0,
      plug: 0,
      arrowAlpha: 0,
      holdUp: 0,
      holdShutter: 0,
      battery: 0,
      door: 0,
      latch: 0,
      check: 0,
      data: 0,
      duration: 1,
    });
    at(0, 0);
    tl.add(rig, { ...overview, tilt: 0.35, laptop: 1, duration: 1100 })
      .add(rig, { cable: 1, duration: 1300, ease: 'inOutSine' }, '-=300')
      .add(rig, { plug: 1, duration: 450, ease: 'outBack' });
    at(1, '+=900');
    tl.add(rig, { ...topPlate, tilt: 1, laptop: 0.25, duration: 1200 })
      .add(rig, { arrowDir: 1, arrow: 0, duration: 1 })
      .add(rig, { arrowAlpha: 1, duration: 200 })
      .add(rig, { arrow: 1, duration: 800, ease: 'inOutQuad' })
      .add(rig, { lever: 1, duration: 450, ease: 'outBack' }, '-=350')
      .add(rig, { lcd: 0, duration: 300 }, '-=150')
      .add(rig, { arrowAlpha: 0, duration: 400 }, '+=500');
    at(2, '+=100');
    tl.add(rig, { ...body, tilt: 0.85, laptop: 0, duration: 1300 })
      .add(rig, { holdUp: 1, duration: 400 }, '-=200')
      .add(rig, { holdShutter: 1, duration: 400 }, '+=500');
    at(3, '+=1300');
    tl.add(rig, { ...holdView, duration: 1100 })
      .add(rig, { arrowDir: -1, arrow: 0, duration: 1 })
      .add(rig, { arrowAlpha: 1, duration: 200 })
      .add(rig, { arrow: 1, duration: 800, ease: 'inOutQuad' })
      .add(rig, { lever: 0, duration: 450, ease: 'outBack' }, '-=350')
      .add(rig, { led: 1, duration: 200 }, '+=200');
    at(4, '+=900');
    tl.add(rig, { holdUp: 0, holdShutter: 0, arrowAlpha: 0, duration: 400 })
      .add(rig, { ...overview, tilt: 0.35, laptop: 1, duration: 1300 }, '+=200')
      .add(rig, { cx: overview.cx, duration: 2400 });
  } else if (step === 'connect') {
    at(0, 0);
    tl.add(rig, {
      ...overview,
      tilt: 0.3,
      laptop: 1,
      cable: 1,
      plug: 1,
      lever: 0,
      lcd: 0,
      led: 1,
      arrowAlpha: 0,
      holdUp: 0,
      holdShutter: 0,
      battery: 0,
      door: 0,
      latch: 0,
      check: 0,
      duration: 1200,
    });
  } else if (step === 'debug') {
    at(0, 0);
    tl.add(rig, { ...overview, tilt: 0.3, laptop: 1, cable: 1, plug: 1, data: 1, duration: 1000 });
  } else {
    tl.add(rig, {
      lever: 0,
      lcd: 0,
      led: 1,
      laptop: 1,
      cable: 1,
      plug: 1,
      battery: 0,
      door: 0,
      latch: 0,
      check: 0,
      data: 0,
      arrowAlpha: 0,
      duration: 1,
    });
    at(0, 0);
    tl.add(rig, { ...baseView, tilt: -1, laptop: 0.25, duration: 1400 }).add(
      rig,
      { latch: 1, duration: 500, ease: 'inOutQuad' },
      '+=200',
    );
    at(1, '+=500');
    tl.add(rig, { door: 1, duration: 800, ease: 'outCubic' })
      .add(rig, { battery: 1, duration: 900, ease: 'inQuad' }, '+=150')
      .add(rig, { led: 0, duration: 100 }, '-=600');
    at(2, '+=900');
    tl.add(rig, { ...overview, tilt: 0.35, laptop: 1, duration: 1300 })
      .add(rig, { plug: 0, duration: 450, ease: 'inBack' }, '+=200')
      .add(rig, { cable: 0, duration: 1000, ease: 'inOutSine' });
    at(3, '+=500');
    tl.add(rig, { ...topPlate, tilt: 1, laptop: 0, duration: 1200 })
      .add(rig, { arrowDir: 1, arrow: 0, duration: 1 })
      .add(rig, { arrowAlpha: 1, duration: 200 })
      .add(rig, { arrow: 1, duration: 700, ease: 'inOutQuad' })
      .add(rig, { lever: 1, duration: 450, ease: 'outBack' }, '-=300')
      .add(rig, { arrowAlpha: 0, duration: 300 }, '+=300')
      .add(rig, { ...baseView, tilt: -1, duration: 1400 })
      .add(rig, { battery: 0, duration: 800, ease: 'outQuad' })
      .add(rig, { door: 0, duration: 700, ease: 'inOutCubic' }, '+=150')
      .add(rig, { latch: 0, duration: 400 });
    at(4, '+=600');
    tl.add(rig, { ...holdView, tilt: 0.9, duration: 1300 })
      .add(rig, { arrowDir: -1, arrow: 0, duration: 1 })
      .add(rig, { arrowAlpha: 1, duration: 200 })
      .add(rig, { arrow: 1, duration: 700, ease: 'inOutQuad' })
      .add(rig, { lever: 0, duration: 450, ease: 'outBack' }, '-=300')
      .add(rig, { lcd: 1, duration: 600 }, '+=200')
      .add(rig, { arrowAlpha: 0, duration: 300 })
      .add(rig, { ...body, check: 1, duration: 900 })
      .add(rig, { cx: body.cx, duration: 2000 });
  }
  return tl;
}

/** The caption due at the timeline's current moment. */
export function beatAt(tl: Timeline) {
  const t = tl.iterationCurrentTime;
  let beat = 0;
  for (const [name, at] of Object.entries(tl.labels)) {
    const i = Number(name.slice(1));
    if (name.startsWith('b') && at <= t + 1 && i > beat) beat = i;
  }
  return beat;
}
