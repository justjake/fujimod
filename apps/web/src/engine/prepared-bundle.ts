/** Load the explicit cold media inputs of a native prepared camera bundle. */
export async function loadPreparedBundle(prepared: unknown, base: URL) {
  if (prepared === null || typeof prepared !== 'object')
    throw Error('Prepared camera has no bundle files.');
  const file = (key: string, value: unknown) => {
    if (typeof value !== 'string' || value.length === 0)
      throw Error(`Prepared camera has an invalid ${key} input.`);
    return new URL(value, base);
  };
  const path = (key: string) => {
    if (!(key in prepared)) throw Error(`Prepared camera has no ${key} input.`);
    return file(key, Reflect.get(prepared, key));
  };
  const urls = ['document', 'bootRom'].map(path);
  const optional = (key: string) => (key in prepared ? path(key) : undefined);
  const nandUrl = optional('nand'),
    settingsUrl = optional('settings');
  // The camera's settings bank sections stay zstd-compressed; the emulator decodes them.
  const declared: unknown = Reflect.get(prepared, 'camera');
  if (declared !== undefined && (declared === null || typeof declared !== 'object'))
    throw Error('Prepared camera has an invalid camera input.');
  const sections = Object.entries(declared ?? {}).map(
    ([section, name]: [string, unknown]) => [section, file(`camera ${section}`, name)] as const,
  );
  const fetched = async (url: URL | undefined) => {
    if (!url) return undefined;
    const response = await fetch(url);
    if (!response.ok) throw Error(`Prepared camera input is unavailable: ${url.pathname}`);
    return response;
  };
  const [[document, bootRom, nand, settings], camera] = await Promise.all([
    Promise.all([...urls, nandUrl, settingsUrl].map(fetched)),
    Promise.all(
      sections.map(async ([section, url]) => {
        const response = await fetched(url);
        return [section, new Uint8Array(await response!.arrayBuffer())] as const;
      }),
    ),
  ]);
  if (!document || !bootRom) throw Error('Prepared camera inputs are incomplete.');
  if (nand && !nand.body) throw Error('Prepared camera NAND input is empty.');
  const [text, bootRomBytes, nandBytes, settingsBytes] = await Promise.all([
    document.text(),
    bootRom.arrayBuffer(),
    nand?.body
      ? new Response(nand.body.pipeThrough(new DecompressionStream('gzip'))).arrayBuffer()
      : undefined,
    settings?.arrayBuffer(),
  ]);
  const inputs = {
    bootRom: new Uint8Array(bootRomBytes),
    nand: nandBytes ? new Uint8Array(nandBytes) : undefined,
    settings: settingsBytes ? new Uint8Array(settingsBytes) : undefined,
    camera: sections.length ? Object.fromEntries(camera) : undefined,
  };
  return { text, inputs };
}
