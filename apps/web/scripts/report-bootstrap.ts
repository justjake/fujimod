// Runs before application modules; records failures that prevent their reporter starting.
(() => {
  let count = 0;
  function report(error: unknown) {
    if (document.documentElement.dataset.reportsReady === 'true' || count++ >= 8) return;
    void fetch('/__simulator-reports', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        id: crypto.randomUUID(),
        event: 'crash',
        archiveExpected: false,
        source: location.pathname.includes('/dev-tools') ? 'dev-tools' : 'emulator',
        page: location.href,
        createdAt: new Date().toISOString(),
        browser: { userAgent: navigator.userAgent },
        fault: { message: String(error), stack: error instanceof Error ? error.stack : undefined },
        snapshot: { unavailable: 'Application report provider did not initialize.' },
      }),
    }).catch(() => {});
  }
  window.addEventListener('error', (event) => report(event.error ?? event.message));
  window.addEventListener('unhandledrejection', (event) => report(event.reason));
})();
