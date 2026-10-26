import type {} from '@fujimod/emulator-ui/engine/performance';
type BootMemory = { wasmMemoryBytes?: number; peakWasmMemoryBytes?: number; stage: string };
declare global {
  interface WindowEventMap {
    'simulator-download': CustomEvent<{ name: string; bytes: number }>;
    'camera-boot-memory': CustomEvent<BootMemory>;
  }
}
// Injected before app modules by the development server; absent from release builds.
(() => {
  const session = crypto.randomUUID();
  let reports = 0;
  let bootMemory: BootMemory | undefined;
  const clean = (value: unknown) =>
    String(value)
      .replace(/(https?:\/\/[^\s?#]+)[?#][^\s]*/g, '$1')
      .slice(0, 6000);
  const describe = (value: unknown) =>
    value instanceof Error
      ? clean(value.stack || value.message)
      : clean(typeof value === 'string' ? value : JSON.stringify(value));
  function report(kind: string, detail: unknown) {
    if (++reports > 60) return;
    const viewer = document.querySelector<HTMLElement>('#viewer');
    const canvas = document.querySelector<HTMLCanvasElement>('#lcd-source');
    const state = viewer?.dataset;
    const body = JSON.stringify({
      session,
      kind,
      detail: clean(detail),
      elapsed: Math.round(performance.now()),
      userAgent: navigator.userAgent,
      webgpu: !!navigator.gpu,
      page: location.pathname,
      visibility: document.visibilityState,
      renderer: state?.renderer,
      emulatorMemoryBytes:
        window.cameraPerformance?.runtime?.wasmMemoryBytes ?? bootMemory?.wasmMemoryBytes,
      emulatorPeakMemoryBytes:
        window.cameraPerformance?.runtime?.peakWasmMemoryBytes ?? bootMemory?.peakWasmMemoryBytes,
      emulatorBootStage: bootMemory?.stage,
      frames: state?.frames,
      canvas: canvas && {
        width: canvas.width,
        height: canvas.height,
        cssWidth: canvas.clientWidth,
        cssHeight: canvas.clientHeight,
      },
      resources: performance
        .getEntriesByType('resource')
        .filter(
          (entry): entry is PerformanceResourceTiming => entry instanceof PerformanceResourceTiming,
        )
        // Keep page revisions as well as library versions: a stale preload can
        // pair old page code with new HTML before the app initializes.
        .filter((entry) =>
          /\.(wasm|glb)(\?|$)|\/\.vite\/deps\/|\/src\/(?:home\/Page|recovery\/Page|development\/entry-client|App|Document)\.tsx(?:\?|$)/.test(
            entry.name,
          ),
        )
        .map((entry) => ({
          path:
            new URL(entry.name).pathname +
            (/\/\.vite\/deps\/|\/src\//.test(entry.name) ? new URL(entry.name).search : ''),
          duration: Math.round(entry.duration),
          bytes: entry.decodedBodySize,
          transferred: entry.transferSize,
          status: entry.responseStatus,
        })),
    });
    void fetch('/__browser-diagnostics', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body,
      keepalive: true,
    }).catch(() => {});
  }
  for (const level of ['warn', 'error'] as const) {
    const original = console[level];
    console[level] = function (...args: unknown[]) {
      original.apply(console, args);
      try {
        report(`console.${level}`, args.map(describe).join(' '));
      } catch {
        /* Logging must not break rendering. */
      }
    };
  }
  window.addEventListener(
    'error',
    (event) => {
      report(
        'error',
        event.error
          ? describe(event.error)
          : event.message ||
              `Resource failed: ${(event.target instanceof HTMLScriptElement ? event.target.src : event.target instanceof HTMLLinkElement ? event.target.href : undefined) || 'unknown'}`,
      );
    },
    true,
  );
  window.addEventListener('unhandledrejection', (event) =>
    report('unhandledrejection', describe(event.reason)),
  );
  document.addEventListener('visibilitychange', () => report('visibility', ''));
  window.addEventListener('simulator-download', (event) => {
    report('download', `${event.detail.name} (${event.detail.bytes} bytes)`);
  });
  window.addEventListener('camera-boot-memory', (event) => {
    bootMemory = event.detail;
    report('emulator-memory', event.detail.stage);
  });
  report('startup', '');
  for (const seconds of [5, 15, 30, 60]) setTimeout(() => report('checkpoint', ''), seconds * 1000);
})();
