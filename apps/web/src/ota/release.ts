/** A camera's OTA release (`/ota/<FIRMWARE>/latest.json`) and its content-addressed store files. */
export interface OtaFile {
  sha256: string;
  size: number;
}
export interface SdBootV2Release {
  firmware: string;
  version: string;
  /** The release date, `YYYY-MM-DD`, and its full commit, when the manifest lists them. */
  date?: string;
  commit?: string;
  /** File layer paths, such as `/firmware.patch`, mapped to their store files. */
  fs: Record<string, OtaFile>;
}

const digest = /^[0-9a-f]{64}$/;
/** Absolute, `/`-separated, without empty segments, backslashes or control characters. */
const layerPath = (path: string) =>
  /^(?:\/[^/\\]+)+$/.test(path) &&
  !Array.from({ length: path.length }, (_, i) => path.charCodeAt(i)).some((code) => code < 0x20);

/** A firmware's release identity: the firmware id in upper case. */
export function sdBootV2Firmware(id: string) {
  return id.toUpperCase();
}

/**
 * A release's card directory under `FUJIMOD/SYSTEM/`: an 8.3 name, which the
 * camera's own file system needs. It is the release identity when that has at
 * most eight characters, otherwise the identity without its hyphens
 * (`X100T-111` uses `X100T111`).
 */
export function sdBootV2SystemFolder(firmware: string) {
  const folder = firmware.length <= 8 ? firmware : firmware.replaceAll('-', '');
  if (!folder || folder.length > 8 || folder.includes('.'))
    throw Error(`The ${firmware} release has no SD card folder name.`);
  return folder;
}

/** The 8.3 store name: the first eight SHA-256 hex digits, a dot and the next three, upper case. */
export function storeName(sha256: string) {
  if (!digest.test(sha256)) throw Error('A store file digest is invalid.');
  return `${sha256.slice(0, 8)}.${sha256.slice(8, 11)}`.toUpperCase();
}

export function validateOtaRelease(value: unknown, firmware: string): SdBootV2Release {
  const release = value as SdBootV2Release;
  if (
    !release ||
    release.firmware !== firmware ||
    typeof release.version !== 'string' ||
    !release.version ||
    release.version.length > 127 ||
    !release.fs ||
    typeof release.fs !== 'object' ||
    Array.isArray(release.fs) ||
    Object.keys(release.fs).length > 32 ||
    (release.date !== undefined && !/^\d{4}-\d{2}-\d{2}$/.test(release.date)) ||
    (release.commit !== undefined && !/^[0-9a-f]{1,40}$/.test(release.commit))
  )
    throw Error(`The ${firmware} release manifest is invalid.`);
  const names = new Map<string, string>();
  for (const [path, file] of Object.entries(release.fs)) {
    if (
      path.length > 127 ||
      !layerPath(path) ||
      path.split('/').some((part) => part === '.' || part === '..') ||
      !file ||
      typeof file.sha256 !== 'string' ||
      !digest.test(file.sha256.toLowerCase()) ||
      !Number.isSafeInteger(file.size) ||
      file.size < 0 ||
      file.size > 0xffffffff
    )
      throw Error(`The ${firmware} release manifest is invalid.`);
    const sha = file.sha256.toLowerCase(),
      name = storeName(sha);
    if (names.has(name) && names.get(name) !== sha)
      throw Error(`The ${firmware} release names two different files ${name}.`);
    names.set(name, sha);
  }
  return {
    firmware,
    version: release.version,
    ...(release.date && { date: release.date }),
    ...(release.commit && { commit: release.commit }),
    fs: Object.fromEntries(
      Object.entries(release.fs).map(([path, file]) => [
        path,
        { sha256: file.sha256.toLowerCase(), size: file.size },
      ]),
    ),
  };
}

export async function loadOtaRelease(
  assetBase: string | URL,
  firmware: string,
  fetcher: typeof fetch = fetch,
): Promise<SdBootV2Release> {
  const asset = new URL(encodeURIComponent(`${firmware}-latest.json`), assetBase);
  const response = await fetcher(asset);
  const unavailable = Error(`The ${firmware} release files are unavailable.`);
  if (!response.ok || !response.headers.get('content-type')?.includes('json')) throw unavailable;
  return validateOtaRelease(
    await response.json().catch(() => Promise.reject(unavailable)),
    firmware,
  );
}

/** The first-install system manifest, `FUJIMOD/SYSTEM/<folder>/A.JSN`. */
export function sdBootV2FirstInstallManifest(release: SdBootV2Release) {
  return {
    gen: 1,
    version: release.version,
    ...(release.date && { date: release.date }),
    ...(release.commit && { commit: release.commit }),
    fs: Object.fromEntries(
      Object.entries(release.fs)
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        .map(([path, file]) => [path, file.sha256]),
    ),
  };
}

/** The first-install boot state, `FUJIMOD/SYSTEM/<folder>/STATE.JSN`. */
export function sdBootV2FirstInstallState() {
  return {
    a: { gen: 1, attempts: 0, confirmed: false },
    b: { gen: -1, attempts: 0, confirmed: false },
    stage2: { attempts: 0, confirmed: false },
  };
}
