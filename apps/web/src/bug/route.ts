import type { Plugin, ViteDevServer, PreviewServer } from 'vite';
/** Preserve the fragment when /bug resolves to its directory entry point. */
export function bugReportRoute() {
  const install = (server: ViteDevServer | PreviewServer) => {
    server.middlewares.use((request, response, next) => {
      const url = request.url ?? '';
      if (url !== '/bug' && !url.startsWith('/bug?')) return next();
      response.statusCode = 307;
      response.setHeader('Location', '/bug/' + url.slice(4));
      response.end();
    });
  };
  return {
    name: 'bug-report-route',
    configureServer: install,
    configurePreviewServer: install,
  } satisfies Plugin;
}
