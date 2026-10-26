import { readFile, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import type { Plugin } from 'vite';

/** A standalone development review; the application router does not own this URL. */
export function powerComboReview(): Plugin {
  return {
    name: 'power-combo-review',
    async writeBundle(options) {
      if (options.dir)
        await rm(resolve(options.dir, 'review/power-combos/index.html'), { force: true });
    },
    configureServer(server) {
      server.middlewares.use(async (request, response, next) => {
        if (request.url?.split('?')[0] !== '/review/power-combos/') return next();
        const html = await readFile(
          resolve(server.config.root, 'public/review/power-combos/index.html'),
          'utf8',
        );
        response.setHeader('Content-Type', 'text/html');
        response.end(await server.transformIndexHtml('/review/power-combos/', html));
      });
    },
  };
}
