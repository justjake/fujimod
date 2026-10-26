import type { Scene } from '@fujimod/emulator-ui/engine/api';

/** Load the X70 sensor's original mosaic without a browser image conversion. */
export async function loadRafScene(): Promise<Scene> {
  const response = await fetch(
    new URL('../../../../build/assets/default-scene.raf.gzipdata', import.meta.url),
  );
  if (!response.ok || !response.body) throw Error('Default RAF is unavailable.');
  const raf = response.body.pipeThrough(new DecompressionStream('gzip'));
  return { raf: new Uint8Array(await new Response(raf).arrayBuffer()) };
}

/** Decode a browser image for the ordinary RGB input path. */
export async function loadImageScene(url: URL): Promise<Scene> {
  const response = await fetch(url);
  if (!response.ok) throw Error('Default scene image is unavailable.');
  const image = await createImageBitmap(await response.blob());
  try {
    const { width, height } = image;
    const context = new OffscreenCanvas(width, height).getContext('2d');
    if (!context) throw Error('Default scene image could not be decoded.');
    context.drawImage(image, 0, 0);
    const rgba = context.getImageData(0, 0, width, height).data;
    const rgb = new Uint8Array(width * height * 3);
    for (let source = 0, target = 0; source < rgba.length; source += 4) {
      rgb[target++] = rgba[source];
      rgb[target++] = rgba[source + 1];
      rgb[target++] = rgba[source + 2];
    }
    return { width, height, rgb };
  } finally {
    image.close();
  }
}
