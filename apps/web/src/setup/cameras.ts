/**
 * Cameras the setup wizard can draw and explain. Every body is drawn from the same
 * named parts, so one set of animations serves all of them (SETUP-WIZARD.md).
 *
 * Geometry is in millimetres, seen from the back:
 * - rear face: x 0..width left to right, y 0..height top to bottom;
 * - top and bottom faces: u along the width (same as x), v from the rear edge (0)
 *   to the front edge (depth); the lens continues past the front edge.
 */
export interface Dial {
  u: number;
  v: number;
  r: number;
  label?: string;
}
export interface Key {
  x: number;
  y: number;
  w: number;
  h: number;
  label?: string;
}
export interface CameraArt {
  width: number;
  height: number;
  depth: number;
  /** Lens barrel on the top/bottom view: width across, length past the front edge, centre u. */
  lens: { u: number; w: number; length: number };
  top: {
    shutter: Dial;
    /** Power lever around the shutter: angle in degrees (0 = toward the rear) for ON and OFF. */
    lever: { r: number; on: number; off: number };
    dials: Dial[];
    shoe: { u: number; w: number };
    flash?: { u: number; w: number };
  };
  rear: {
    lcd: Key;
    evf?: Key;
    dpad: { x: number; y: number; r: number };
    keys: Key[];
    led: { x: number; y: number };
    grip?: { x: number; w: number };
  };
  /** USB port on the right-hand side, as a fraction of the height. */
  usb: { y: number };
  bottom: {
    battery: { u: number; v: number; w: number; d: number };
    tripod: { u: number; v: number };
  };
}
export interface Camera {
  id: 'x70' | 'xm1' | 'xe2s';
  name: string;
  art: CameraArt;
  /** Keys held while switching on, all three bodies: USB connected + UP + full shutter. */
  entry: { reviewed: boolean; note: string };
  /** Exit guard the page arms in service mode, where one is known. */
  guard: 'known' | 'candidate' | 'none';
}

export const cameras: Camera[] = [
  {
    id: 'x70',
    name: 'X70',
    entry: {
      reviewed: true,
      note: 'Tried on an X70: the screen stays dark and the green light comes on.',
    },
    guard: 'known',
    art: {
      width: 113,
      height: 64,
      depth: 40,
      lens: { u: 46, w: 48, length: 12 },
      top: {
        shutter: { u: 92, v: 20, r: 5.5 },
        lever: { r: 9.5, on: 205, off: 245 },
        dials: [
          { u: 106, v: 18, r: 7, label: '±' },
          { u: 75, v: 22, r: 8, label: 'S' },
        ],
        shoe: { u: 34, w: 20 },
      },
      rear: {
        lcd: { x: 6, y: 9, w: 70, h: 47 },
        dpad: { x: 96, y: 37, r: 9 },
        keys: [
          { x: 84, y: 9, w: 9, h: 5, label: 'AE' },
          { x: 99, y: 9, w: 9, h: 5, label: 'Q' },
          { x: 84, y: 51, w: 9, h: 5, label: '▶' },
          { x: 99, y: 51, w: 9, h: 5, label: 'DISP' },
        ],
        led: { x: 81, y: 18 },
        grip: { x: 80, w: 33 },
      },
      usb: { y: 0.55 },
      bottom: { battery: { u: 82, v: 8, w: 24, d: 26 }, tripod: { u: 46, v: 20 } },
    },
  },
  {
    id: 'xm1',
    name: 'X-M1',
    entry: {
      reviewed: false,
      note: 'Inferred from the X-M1 firmware; not yet tried on an X-M1. Its exit has not been reviewed.',
    },
    guard: 'none',
    art: {
      width: 117,
      height: 67,
      depth: 39,
      lens: { u: 48, w: 54, length: 30 },
      top: {
        shutter: { u: 99, v: 24, r: 5 },
        lever: { r: 9, on: 210, off: 250 },
        dials: [
          { u: 84, v: 20, r: 8.5, label: 'P' },
          { u: 108, v: 9, r: 5, label: '' },
        ],
        shoe: { u: 48, w: 20 },
        flash: { u: 28, w: 18 },
      },
      rear: {
        lcd: { x: 6, y: 10, w: 73, h: 50 },
        dpad: { x: 101, y: 40, r: 9 },
        keys: [
          { x: 88, y: 10, w: 9, h: 5, label: 'AE' },
          { x: 103, y: 10, w: 9, h: 5, label: 'Q' },
          { x: 88, y: 55, w: 9, h: 5, label: '▶' },
          { x: 103, y: 55, w: 9, h: 5, label: 'DISP' },
        ],
        led: { x: 85, y: 20 },
        grip: { x: 84, w: 33 },
      },
      usb: { y: 0.5 },
      bottom: { battery: { u: 88, v: 8, w: 22, d: 26 }, tripod: { u: 48, v: 20 } },
    },
  },
  {
    id: 'xe2s',
    name: 'X-E2S',
    entry: {
      reviewed: false,
      note: 'Inferred from the X-E2S firmware; not yet tried on an X-E2S. Its exit protection is unreviewed.',
    },
    guard: 'candidate',
    art: {
      width: 129,
      height: 75,
      depth: 37,
      lens: { u: 52, w: 58, length: 34 },
      top: {
        shutter: { u: 96, v: 26, r: 5 },
        lever: { r: 9, on: 165, off: 125 },
        dials: [
          { u: 96, v: 12, r: 9, label: 'S' },
          { u: 117, v: 13, r: 8, label: '±' },
        ],
        shoe: { u: 52, w: 20 },
      },
      rear: {
        lcd: { x: 8, y: 22, w: 72, h: 46 },
        evf: { x: 6, y: 5, w: 22, h: 14 },
        dpad: { x: 106, y: 47, r: 9.5 },
        keys: [
          { x: 92, y: 24, w: 10, h: 5, label: 'AE' },
          { x: 110, y: 24, w: 10, h: 5, label: 'Q' },
          { x: 92, y: 63, w: 10, h: 5, label: '▶' },
          { x: 110, y: 63, w: 10, h: 5, label: 'DISP' },
        ],
        led: { x: 31, y: 9 },
      },
      usb: { y: 0.55 },
      bottom: { battery: { u: 98, v: 8, w: 24, d: 25 }, tripod: { u: 52, v: 20 } },
    },
  },
];
