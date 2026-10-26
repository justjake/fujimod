import type { Plugin } from 'vite';
import { browserScript } from './browser-script.ts';
import { appendFile, mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

export function browserDiagnostics(): Plugin {
  return {
    name: 'browser-diagnostics',
    apply: 'serve',
    configureServer(server) {
      const directory = resolve(server.config.root, 'build/web-service');
      server.middlewares.use((req, res, next) => {
        if (req.url === '/__browser-diagnostics.js' && req.method === 'GET') {
          res.setHeader('Content-Type', 'text/javascript');
          res.setHeader('Cache-Control', 'no-store');
          void browserScript(new URL('./browser-diagnostics-client.ts', import.meta.url))
            .then((source) => res.end(source))
            .catch(next);
          return;
        }
        if (req.url !== '/__browser-diagnostics') return next();
        if (req.method !== 'POST') {
          res.statusCode = 405;
          res.end();
          return;
        }
        try {
          if (req.headers.origin && new URL(req.headers.origin).host !== req.headers.host)
            throw Error('Cross-origin report');
        } catch {
          res.statusCode = 403;
          res.end();
          return;
        }
        let body = '',
          bytes = 0;
        req.on('data', (chunk) => {
          bytes += chunk.length;
          if (bytes <= 32768) body += chunk;
        });
        req.on('end', () => {
          if (bytes > 32768) {
            res.statusCode = 413;
            res.end();
            return;
          }
          let report;
          try {
            report = JSON.parse(body);
            if (typeof report.session !== 'string' || typeof report.kind !== 'string')
              throw Error('Invalid report');
          } catch {
            res.statusCode = 400;
            res.end();
            return;
          }
          const line = JSON.stringify({ received: new Date().toISOString(), ...report });
          void mkdir(directory, { recursive: true })
            .then(() => appendFile(resolve(directory, 'browser-diagnostics.jsonl'), line + '\n'))
            .then(() => {
              server.config.logger.info(`[browser] ${line}`);
              res.statusCode = 204;
              res.end();
            })
            .catch(next);
        });
      });
    },
  };
}
