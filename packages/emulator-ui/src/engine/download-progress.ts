export interface AssetProgress {
  url: string;
  label: string;
  loaded: number;
  total?: number;
  unit?: 'instructions';
  status: 'waiting' | 'loading' | 'complete' | 'failed';
  rate?: number;
  remaining?: number;
}
export type LoadingProgress = (message: string, percent?: number, assets?: AssetProgress[]) => void;

interface Download extends AssetProgress {
  started: number;
}
const mib = (bytes: number) => (bytes / 1048576).toFixed(1);
function label(url: string) {
  const name = new URL(url).pathname.split('/').at(-1) ?? 'asset';
  return (
    (
      {
        'wasm.json': 'Runtime manifest',
        'camera.mjs': 'Runtime loader',
        'camera.wasm': 'Camera emulator',
        'firmware.dat': 'Camera firmware',
      } as Record<string, string>
    )[name] ?? name
  );
}

/** A non-download loading step: verification, patching, construction or boot. */
export function loadingStep(
  id: 'verify' | 'patch' | 'build' | 'boot',
  status: AssetProgress['status'],
  bytes?: number,
): AssetProgress {
  return {
    url: `step:${id}`,
    label: {
      verify: 'Verifying firmware',
      patch: 'Patching firmware',
      build: 'Building camera',
      boot: 'Booting camera',
    }[id],
    loaded: bytes ?? 0,
    total: bytes,
    status,
  };
}
/** Guest work, distinct from download bytes; completion belongs to REC observation. */
export function bootProgress(instructions: number, total?: number): AssetProgress {
  return { ...loadingStep('boot', 'loading'), loaded: instructions, total, unit: 'instructions' };
}
/** Every row of a boot, in order, before any starts; a download with the same label replaces its placeholder. */
export function expectedAssets(patch: boolean): AssetProgress[] {
  return [
    loadingStep('verify', 'waiting'),
    ...['wasm.json', 'camera.wasm', 'camera.mjs'].map((name): AssetProgress => ({
      url: `expected:${name}`,
      label: label(`https://expected.invalid/${name}`),
      loaded: 0,
      status: 'waiting',
    })),
    ...(patch ? [loadingStep('patch', 'waiting')] : []),
    loadingStep('build', 'waiting'),
    loadingStep('boot', 'waiting'),
  ];
}

/** Observe consumed response bytes without buffering or delaying the download.
 * Retain every asset through setup, including requests waiting for headers and completed reads. */
export class DownloadProgress {
  private active = new Set<Download>();
  private downloads = new Map<string, Download>();
  private lastReport = -Infinity;
  constructor(
    private report: LoadingProgress,
    private now: () => number = () => performance.now(),
  ) {}
  private begin(url: string) {
    const download: Download = {
      url,
      label: label(url),
      loaded: 0,
      started: this.now(),
      status: 'waiting',
    };
    this.downloads.set(url, download);
    this.active.add(download);
    this.update(true);
    return download;
  }
  async fetch(url: string, fetcher: () => Promise<Response>) {
    const download = this.begin(url);
    try {
      return this.response(url, await fetcher());
    } catch (error) {
      this.fail(download);
      throw error;
    }
  }
  async module<T>(url: string, load: () => Promise<T>): Promise<T> {
    const download = this.begin(url);
    try {
      const value = await load();
      const entry = performance.getEntriesByName(url).at(-1) as
        | PerformanceResourceTiming
        | undefined;
      download.loaded = entry?.decodedBodySize ?? 0;
      download.total = download.loaded || undefined;
      this.complete(download);
      return value;
    } catch (error) {
      this.fail(download);
      throw error;
    }
  }
  private fail(download: Download) {
    download.status = 'failed';
    this.active.delete(download);
    this.report('Asset loading failed', undefined, this.snapshot());
  }
  private complete(download: Download) {
    download.status = 'complete';
    download.remaining = undefined;
    this.active.delete(download);
    if (this.active.size) this.update(true);
    else this.report('Initializing camera runtime…', undefined, this.snapshot());
  }
  private snapshot(): AssetProgress[] {
    return Array.from(this.downloads.values(), ({ started: _started, ...asset }) => ({ ...asset }));
  }
  response(url: string, response: Response): Response {
    if (!response.ok || !response.body) {
      const pending = this.downloads.get(url);
      if (pending) {
        if (response.ok) this.complete(pending);
        else this.fail(pending);
      }
      return response;
    }
    const decodedSize = Number(response.headers.get('x-decoded-content-length'));
    const size = Number(response.headers.get('content-length'));
    const encoding = response.headers.get('content-encoding');
    const existing = this.downloads.get(url);
    const download = existing?.status === 'waiting' ? existing : this.begin(url);
    download.status = 'loading';
    // Fetch decodes compressed bodies; wire length is not a decoded-byte total.
    download.total =
      Number.isSafeInteger(decodedSize) && decodedSize > 0
        ? decodedSize
        : Number.isSafeInteger(size) && size > 0 && (!encoding || encoding === 'identity')
          ? size
          : undefined;
    this.update(true);
    const reader = response.body.getReader();
    const body = new ReadableStream<Uint8Array>({
      pull: async (controller) => {
        try {
          const { done, value } = await reader.read();
          if (done) {
            this.update(true);
            this.complete(download);
            controller.close();
          } else {
            download.loaded += value.byteLength;
            this.update(false);
            controller.enqueue(value);
          }
        } catch (error) {
          this.fail(download);
          controller.error(error);
        }
      },
      cancel: async (reason) => {
        this.fail(download);
        await reader.cancel(reason);
      },
    });
    return new Response(body, {
      status: response.status,
      statusText: response.statusText,
      headers: response.headers,
    });
  }
  private update(force: boolean) {
    const time = this.now();
    if (!force && time - this.lastReport < 100) return;
    const current = this.active.values().next().value;
    if (!current) return;
    this.lastReport = time;
    const seconds = (time - current.started) / 1000;
    const rate = seconds >= 1 && current.loaded > 0 ? current.loaded / seconds : undefined;
    const total = current.total && current.loaded <= current.total ? current.total : undefined;
    const remaining = total && rate ? Math.ceil((total - current.loaded) / rate) : undefined;
    for (const item of this.active) {
      const elapsed = (time - item.started) / 1000;
      item.rate = elapsed >= 1 && item.loaded > 0 ? item.loaded / elapsed : undefined;
      item.remaining =
        item.total && item.rate
          ? Math.max(0, Math.ceil((item.total - item.loaded) / item.rate))
          : undefined;
    }
    const count = `${mib(current.loaded)}${total ? ` / ${mib(total)}` : ''} MiB`;
    this.report(
      `${current.label} · ${count}${rate ? ` · ${mib(rate)} MiB/s` : ''}${remaining !== undefined ? ` · ~${remaining} s remaining` : ''}`,
      total ? Math.min(100, (100 * current.loaded) / total) : undefined,
      this.snapshot(),
    );
  }
}
