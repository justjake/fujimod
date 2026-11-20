import type { ReportEntry } from './report-link';

/** A report's saved display planes, as its manifest describes them. Zero values are left out. */
export interface ReportScreen {
  /** Y0 Y1 Cb Cr for each pixel pair. */
  video: string;
  width: number;
  height: number;
  /** A B G R bytes per pixel, stacked from the stock OSD up. */
  layers?: ScreenLayer[];
  rectangles?: ScreenRectangle[];
}
export interface ScreenLayer {
  path: string;
  x?: number;
  y?: number;
  width?: number;
  height?: number;
  /** Global alpha out of 128. */
  alpha?: number;
}
export interface ScreenRectangle {
  left?: number;
  top?: number;
  right?: number;
  bottom?: number;
  thickness?: number;
  inset_x?: number;
  inset_y?: number;
  luma?: number;
  /** Centered chroma. */
  cb?: number;
  cr?: number;
  /** Out of 128. */
  opacity?: number;
  corners?: boolean;
}

/** The display's YCbCr conversion: round half to even, clamp to 0..255. */
function channel(numerator: number, denominator: number) {
  if (numerator < 0) return 0;
  let whole = Math.floor(numerator / denominator);
  const remainder = numerator - whole * denominator;
  if (remainder * 2 > denominator || (remainder * 2 === denominator && whole & 1)) whole++;
  return Math.min(whole, 255);
}
function rgb(luma: number, cb: number, cr: number): [number, number, number] {
  return [
    channel(luma * 500 + cr * 701, 500),
    channel(luma * 587000 - cb * 202008 - cr * 419198, 587000),
    channel(luma * 250 + cb * 443, 250),
  ];
}
function covers(r: Required<ScreenRectangle>, x: number, y: number) {
  if (x < r.left || x > r.right || y < r.top || y > r.bottom) return false;
  const horizontal = y - r.top < r.thickness || r.bottom - y < r.thickness;
  const vertical = x - r.left < r.thickness || r.right - x < r.thickness;
  if (!r.corners) return horizontal || vertical;
  return (
    (horizontal && (x - r.left < r.inset_x || r.right - x < r.inset_x)) ||
    (vertical && (y - r.top < r.inset_y || r.bottom - y < r.inset_y))
  );
}

/** Composites the planes into RGB bytes, or undefined when a plane is missing or the wrong size. */
export function composeScreen(
  screen: ReportScreen,
  file: (path: string) => Uint8Array | undefined,
): Uint8Array | undefined {
  const { width, height } = screen;
  const video = file(screen.video);
  if (!(width > 0 && height > 0 && width % 2 === 0) || video?.length !== width * height * 2)
    return undefined;
  const image = new Uint8Array(width * height * 3);
  for (let i = 0; i < width * height; i += 2) {
    const cb = video[i * 2 + 2] - 128,
      cr = video[i * 2 + 3] - 128;
    image.set(rgb(video[i * 2], cb, cr), i * 3);
    image.set(rgb(video[i * 2 + 1], cb, cr), i * 3 + 3);
  }
  const denominator = 255 * 128;
  for (const layer of screen.layers ?? []) {
    const { x = 0, y = 0, width: w = 0, height: h = 0, alpha = 0 } = layer;
    const pixels = file(layer.path);
    if (pixels?.length !== w * h * 4 || x + w > width || y + h > height) return undefined;
    for (let row = 0; row < h; row++)
      for (let column = 0; column < w; column++) {
        const source = (row * w + column) * 4,
          target = ((y + row) * width + x + column) * 3;
        const weight = pixels[source] * alpha;
        for (let c = 0; c < 3; c++)
          image[target + c] = Math.floor(
            (pixels[source + 3 - c] * weight +
              image[target + c] * (denominator - weight) +
              denominator / 2) /
              denominator,
          );
      }
  }
  for (const rectangle of screen.rectangles ?? []) {
    const r: Required<ScreenRectangle> = {
      left: 0,
      top: 0,
      right: 0,
      bottom: 0,
      thickness: 0,
      inset_x: 0,
      inset_y: 0,
      luma: 0,
      cb: 0,
      cr: 0,
      opacity: 0,
      corners: false,
      ...rectangle,
    };
    if (!r.opacity) continue;
    const color = rgb(r.luma, r.cb, r.cr);
    for (let y = r.top; y <= Math.min(r.bottom, height - 1); y++)
      for (let x = r.left; x <= Math.min(r.right, width - 1); x++) {
        if (!covers(r, x, y)) continue;
        const target = (y * width + x) * 3;
        for (let c = 0; c < 3; c++)
          image[target + c] = Math.floor(
            (color[c] * r.opacity + image[target + c] * (128 - r.opacity) + 64) / 128,
          );
      }
  }
  return image;
}

/** A top-down 24-bit BMP of RGB bytes. */
export function bitmap(
  rgbBytes: Uint8Array,
  width: number,
  height: number,
): Uint8Array<ArrayBuffer> {
  const stride = Math.ceil((width * 3) / 4) * 4,
    size = 54 + stride * height;
  const out = new Uint8Array(size),
    view = new DataView(out.buffer);
  out.set([0x42, 0x4d]);
  view.setUint32(2, size, true);
  view.setUint32(10, 54, true);
  view.setUint32(14, 40, true);
  view.setInt32(18, width, true);
  view.setInt32(22, -height, true);
  view.setUint16(26, 1, true);
  view.setUint16(28, 24, true);
  view.setUint32(34, stride * height, true);
  for (let y = 0; y < height; y++)
    for (let x = 0; x < width; x++) {
      const source = (y * width + x) * 3,
        target = 54 + y * stride + x * 3;
      out[target] = rgbBytes[source + 2];
      out[target + 1] = rgbBytes[source + 1];
      out[target + 2] = rgbBytes[source];
    }
  return out;
}

/** The report's screen as a BMP, when its manifest describes one and every plane is present. */
export function screenBitmap(
  bytes: Uint8Array<ArrayBuffer>,
  entries: readonly ReportEntry[],
  screen: ReportScreen | undefined,
): Uint8Array<ArrayBuffer> | undefined {
  if (!screen?.video) return undefined;
  const image = composeScreen(screen, (path) => {
    const entry = entries.find((candidate) => candidate.name === path);
    return entry && bytes.subarray(entry.offset, entry.offset + entry.bytes);
  });
  return image && bitmap(image, screen.width, screen.height);
}
