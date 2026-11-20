/** The local sharing Worker the development preview starts with wrangler dev. */
export const SHARING_WORKER_ORIGIN = 'http://127.0.0.1:8787';

/** Worker paths the preview serves under its own origin. */
export const SHARING_WORKER_PATHS = [
  '/proxy',
  '/health',
  '/error-logs',
  '/issue-reports',
  '/contributions',
  '/contribution',
  '/file-viewer.css',
];
