import type { ViteDevServer, PreviewServer } from 'vite';

/** Serves `/name` from its prerendered `/name/` entry, preserving the query and fragment. */
export function directoryRoute(directory: string) {
  const install = (server: ViteDevServer | PreviewServer) => {
    server.middlewares.use((request, response, next) => {
      const url = request.url ?? '';
      if (url !== `/${directory}` && !url.startsWith(`/${directory}?`)) return next();
      response.statusCode = 307;
      response.setHeader('Location', `/${directory}/` + url.slice(directory.length + 1));
      response.end();
    });
  };
  return {
    name: `${directory}-directory-route`,
    configureServer: install,
    configurePreviewServer: install,
  };
}
