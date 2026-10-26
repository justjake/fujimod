/** Runtime Wasm compilation is allowed; JavaScript eval remains disabled. Cloudflare
 * Web Analytics injects its beacon from static.cloudflareinsights.com. */
export const contentSecurityPolicy =
  "script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' https://static.cloudflareinsights.com; worker-src 'self' blob:; object-src 'none'; base-uri 'self'";

/** Shared memory is available to the emulator owner and its device pthreads. */
export const runtimeHeaders = {
  'Content-Security-Policy': contentSecurityPolicy,
  'Cross-Origin-Opener-Policy': 'same-origin',
  'Cross-Origin-Embedder-Policy': 'require-corp',
} as const;
