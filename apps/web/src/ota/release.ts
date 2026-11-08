/** A camera's OTA release (`/ota/<FIRMWARE>/latest.json`) and its content-addressed store files. */
export interface OtaFile {
  sha256: string;
  size: number;
}
export interface OtaRelease {
  firmware: string;
  version: string;
  /** File layer paths, such as `/firmware.patch`, mapped to their store files. */
  fs: Record<string, OtaFile>;
}

const digest = /^[0-9a-f]{64}$/;
/** Absolute, `/`-separated, without empty segments, backslashes or control characters. */
const layerPath = (path: string) =>
  /^(?:\/[^/\\]+)+$/.test(path) &&
  !Array.from({ length: path.length }, (_, i) => path.charCodeAt(i)).some((code) => code < 0x20);

/** The card directory of a firmware's release: the firmware id in upper case. */
export function otaFirmware(id: string) {
  return id.toUpperCase();
}

/** The 8.3 store name: the first eight SHA-256 hex digits, a dot and the next three, upper case. */
export function storeName(sha256: string) {
  if (!digest.test(sha256)) throw Error('A store file digest is invalid.');
  return `${sha256.slice(0, 8)}.${sha256.slice(8, 11)}`.toUpperCase();
}

export function validateOtaRelease(value: unknown, firmware: string): OtaRelease {
  const release = value as OtaRelease;
  if (
    !release ||
    release.firmware !== firmware ||
    typeof release.version !== 'string' ||
    !release.version ||
    release.version.length > 127 ||
    !release.fs ||
    typeof release.fs !== 'object' ||
    Array.isArray(release.fs) ||
    Object.keys(release.fs).length > 32
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
    fs: Object.fromEntries(
      Object.entries(release.fs).map(([path, file]) => [
        path,
        { sha256: file.sha256.toLowerCase(), size: file.size },
      ]),
    ),
  };
}

export async function loadOtaRelease(
  base: string | URL,
  firmware: string,
  fetcher: typeof fetch = fetch,
): Promise<OtaRelease> {
  const response = await fetcher(new URL(`/ota/${encodeURIComponent(firmware)}/latest.json`, base));
  const unavailable = Error(`The ${firmware} release files are unavailable.`);
  if (!response.ok || !response.headers.get('content-type')?.includes('json')) throw unavailable;
  return validateOtaRelease(
    await response.json().catch(() => Promise.reject(unavailable)),
    firmware,
  );
}

/** The first-install system manifest, `FUJIMOD/SYSTEM/<FIRMWARE>/A.JSN`. */
export function firstInstallManifest(release: OtaRelease) {
  return {
    gen: 1,
    version: release.version,
    fs: Object.fromEntries(
      Object.entries(release.fs)
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        .map(([path, file]) => [path, file.sha256]),
    ),
  };
}

/** The first-install boot state, `FUJIMOD/SYSTEM/<FIRMWARE>/STATE.JSN`. */
export function firstInstallState() {
  return {
    a: { gen: 1, attempts: 0, confirmed: false },
    b: { gen: -1, attempts: 0, confirmed: false },
    stage2: { attempts: 0, confirmed: false },
  };
}
