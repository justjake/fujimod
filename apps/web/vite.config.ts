import { prepareDefaultScene } from '../../scripts/assets/default-scene.ts';

import { uiLint } from '../../packages/emulator-ui/lint';
import { runtimeHeaders } from './src/security';
import { defineConfig } from 'vite-plus';
import { loadEnv, normalizePath, type Plugin, type Connect } from 'vite';
import solid from '@solidjs/vite-plugin';
import { wasmProofRoute } from './scripts/wasm-proof-route';
import { simulatorReports } from './scripts/local-reports.ts';
import { bugReportRoute } from './src/bug/route.ts';
import { powerComboReview } from './scripts/power-combo-review';
import { browserDiagnostics } from './scripts/browser-diagnostics.ts';
import { execFileSync } from 'node:child_process';
import { createReadStream, existsSync, readFileSync } from 'node:fs';
import { createGzip } from 'node:zlib';
import { stat } from 'node:fs/promises';
import { resolve, sep } from 'node:path';

await prepareDefaultScene();

// Set these before custom routes can finish a response, including proof pages
// and runtime mounts that bypass Vite's static-response headers.
function isolationHeaders(): Plugin {
  const configure = (middlewares: Connect.Server) => {
    middlewares.use((_request, response, next) => {
      for (const [name, value] of Object.entries(runtimeHeaders)) response.setHeader(name, value);
      next();
    });
  };
  return {
    name: 'runtime-isolation-headers',
    configureServer: (server) => {
      configure(server.middlewares);
    },
    configurePreviewServer: (server) => {
      configure(server.middlewares);
    },
  };
}

// Workspace packages are imported through node_modules links, which Vite's watcher skips;
// watch their sources so edits reach the dev server without a restart.
function watchWorkspacePackages(): Plugin {
  return {
    name: 'watch-workspace-packages',
    configureServer: (server) => {
      server.watcher.add(resolve('../../packages'));
    },
  };
}

// Opt-in local artifact mounts are served by the local server, never copied into dist.
function localArtifacts(env: Record<string, string>): Plugin {
  const configure = (middlewares: Connect.Server) => {
    const firmwares = Object.entries(JSON.parse(env.FUJIMOD_LOCAL_FIRMWARES || '{}'));
    if (firmwares.some(([id, path]) => !/^[a-z0-9-]+$/.test(id) || typeof path !== 'string'))
      throw Error('FUJIMOD_LOCAL_FIRMWARES must map firmware IDs to local DAT paths');
    const mounts = [
      ['/runtime/', env.FUJIMOD_RUNTIME],
      ['/artifacts/docs/', env.FUJIMOD_MEDIA],
      ['/artifacts/', env.FUJIMOD_PATCHES],
      ['/local/firmware.dat', env.FUJIMOD_DAT],
      ...firmwares.map(([id, path]) => [`/local/firmware/${id}.dat`, path as string]),
    ].filter((pair): pair is [string, string] => Boolean(pair[1]));
    middlewares.use(async (req, res, next) => {
      const url = new URL(req.url || '/', 'http://localhost').pathname;
      const mount = mounts.find(([prefix]) => url.startsWith(prefix));
      if (!mount) return next();
      const [prefix, root] = mount;
      let relative: string;
      try {
        relative = decodeURIComponent(url.slice(prefix.length));
      } catch {
        res.statusCode = 400;
        res.end();
        return;
      }
      if (
        prefix === '/runtime/' &&
        !/^(catalog\.json|[a-zA-Z0-9_-]+\/(wasm\.json|camera\.json|flash-reader\.(?:json|bin)|document\.json|boot-rom\.bin|nand-[a-z0-9-]+\.bin\.zst|browser-nand\.bin\.gz|mcu-settings\.bin|(?:bundle\/)?camera\.mjs|(?:bundle\/)?camera\.wasm|runtime-NOTICES\.txt)|wasm-(proof(?:-fault)?|session-proof)\/program\.dat)$/.test(
          relative,
        )
      ) {
        res.statusCode = 404;
        res.end();
        return;
      }
      const path = relative ? resolve(root, relative) : resolve(root);
      if (relative && !path.startsWith(resolve(root) + sep)) {
        res.statusCode = 403;
        res.end();
        return;
      }
      try {
        const info = await stat(path);
        if (!info.isFile()) throw Error('not a file');
        const gzip =
          /\bgzip\b/.test(` ${req.headers['accept-encoding'] || ''}`) &&
          info.size > 65536 &&
          /\.(dat|so|wasm|mjs|js|json)$/i.test(path);
        const etag = `"${info.size.toString(16)}-${info.mtimeMs.toString(16)}${gzip ? '-gzip' : ''}"`;
        res.setHeader('Vary', 'Accept-Encoding');
        res.setHeader('Cache-Control', 'no-cache');
        res.setHeader('ETag', etag);
        if (req.headers['if-none-match'] === etag) {
          res.statusCode = 304;
          res.end();
          return;
        }
        if (gzip) {
          res.setHeader('Content-Encoding', 'gzip');
          res.setHeader('X-Decoded-Content-Length', info.size);
        }
        // Mobile Safari fetches video in byte ranges and stalls without them.
        const range = gzip ? null : /^bytes=(\d*)-(\d*)$/.exec(req.headers.range || '');
        const first = range ? (range[1] ? Number(range[1]) : info.size - Number(range[2])) : 0;
        const last =
          range && range[1] && range[2] ? Math.min(Number(range[2]), info.size - 1) : info.size - 1;
        if (range && (first > last || first < 0)) {
          res.statusCode = 416;
          res.setHeader('Content-Range', `bytes */${info.size}`);
          res.end();
          return;
        }
        if (!gzip) {
          res.setHeader('Content-Length', last - first + 1);
          res.setHeader('Accept-Ranges', 'bytes');
        }
        if (range) {
          res.statusCode = 206;
          res.setHeader('Content-Range', `bytes ${first}-${last}/${info.size}`);
        }
        res.setHeader(
          'Content-Type',
          path.endsWith('.mjs') || path.endsWith('.js')
            ? 'text/javascript'
            : path.endsWith('.json')
              ? 'application/json'
              : path.endsWith('.wasm')
                ? 'application/wasm'
                : path.endsWith('.mp4')
                  ? 'video/mp4'
                  : path.endsWith('.webp')
                    ? 'image/webp'
                    : 'application/octet-stream',
        );
        if (req.method === 'HEAD') {
          res.end();
          return;
        }
        const stream = createReadStream(path, { start: first, end: last });
        (gzip ? stream.pipe(createGzip({ level: 1 })) : stream).pipe(res);
      } catch {
        res.statusCode = 404;
        res.end();
      }
    });
  };
  return {
    name: 'local-artifacts',
    apply: 'serve',
    configureServer: (server) => configure(server.middlewares),
    configurePreviewServer: (server) => configure(server.middlewares),
  };
}
// Feature media by card: an opt-in local directory, else the staged site assets.
function featureMedia(env: Record<string, string>) {
  const path = env.FUJIMOD_MEDIA
    ? resolve(env.FUJIMOD_MEDIA, 'media.json')
    : 'public/artifacts/docs/media.json';
  return existsSync(path) ? readFileSync(path, 'utf8') : 'null';
}

function catalogPath(env: Record<string, string>) {
  return env.FUJIMOD_RUNTIME
    ? resolve(env.FUJIMOD_RUNTIME, 'catalog.json')
    : existsSync('public/runtime/catalog.json')
      ? 'public/runtime/catalog.json'
      : 'public/catalog.json';
}

function simulatedUsbAvailable(env: Record<string, string>): boolean {
  const runtime = resolve(env.FUJIMOD_RUNTIME || 'public/runtime');
  const path = resolve(runtime, 'catalog.json');
  if (!existsSync(path)) return false;
  const catalog = JSON.parse(readFileSync(path, 'utf8')) as {
    firmwares?: { emulate?: { usb?: { bundle: string } } }[];
  };
  return Boolean(
    catalog.firmwares?.some((entry) => {
      const bundle = entry.emulate?.usb?.bundle;
      if (!bundle || !/^runtime\/[a-zA-Z0-9_-]+\/$/.test(bundle)) return false;
      const path = resolve(runtime, bundle.slice('runtime/'.length), 'wasm.json');
      if (!existsSync(path)) return false;
      const manifest = JSON.parse(readFileSync(path, 'utf8')) as {
        schema?: number;
        abi?: number;
        construction?: string;
        usb?: { kind?: string; node?: string };
      };
      return (
        manifest.schema === 1 &&
        manifest.abi === 1 &&
        manifest.construction === 'session' &&
        manifest.usb?.kind === 'exr2' &&
        Boolean(manifest.usb.node)
      );
    }),
  );
}
export default defineConfig(({ mode, command }) => ({
  base: '/',
  resolve: { dedupe: ['solid-js', '@solidjs/web', '@solidjs/signals'] },
  // Solid's server build consumes the manifest produced by its client build.
  builder: { sharedPlugins: true },
  define: {
    __FUJIMOD_SIMULATED_USB__: simulatedUsbAvailable(loadEnv(mode, process.cwd(), 'FUJIMOD_')),
    __FUJIMOD_BUILD_DAY__: JSON.stringify(new Date().toISOString().slice(0, 10)),
    __FUJIMOD_INITIAL_CATALOG__: readFileSync(
      catalogPath(loadEnv(mode, process.cwd(), 'FUJIMOD_')),
      'utf8',
    ),
    __FUJIMOD_INITIAL_RELEASE__: existsSync('public/artifacts/patches.json')
      ? readFileSync('public/artifacts/patches.json', 'utf8')
      : 'null',
    __FUJIMOD_SIMULATOR_BUILD__: JSON.stringify(
      `${execFileSync('git', ['rev-parse', '--short=12', 'HEAD'], { encoding: 'utf8' }).trim()} / ${new Date().toISOString()}`,
    ),
    __FUJIMOD_SIGNAL_URL__: JSON.stringify(
      loadEnv(mode, process.cwd(), 'FUJIMOD_').FUJIMOD_SIGNAL_URL || '',
    ),
    __FUJIMOD_FEATURE_MEDIA__: featureMedia(loadEnv(mode, process.cwd(), 'FUJIMOD_')),
    __FUJIMOD_LOCAL_DAT__:
      (command === 'serve' || mode === 'local-preview') &&
      Boolean(loadEnv(mode, process.cwd(), 'FUJIMOD_').FUJIMOD_DAT),
    __FUJIMOD_CATALOG__: JSON.stringify(
      catalogPath(loadEnv(mode, process.cwd(), 'FUJIMOD_')).endsWith('runtime/catalog.json') ||
        loadEnv(mode, process.cwd(), 'FUJIMOD_').FUJIMOD_RUNTIME
        ? 'runtime/catalog.json'
        : 'catalog.json',
    ),
  },
  plugins: [
    isolationHeaders(),
    powerComboReview(),
    watchWorkspacePackages(),
    solid({
      start: {
        middleware: './src/middleware.ts',
        ...(command === 'serve'
          ? {
              entryServer: './src/development/entry-server.tsx',
              entryClient: './src/development/entry-client.tsx',
            }
          : {}),
      },
      ssr: true,
    }),
    bugReportRoute(),
    wasmProofRoute(loadEnv(mode, process.cwd(), 'FUJIMOD_')),
    simulatorReports(),
    browserDiagnostics(),
    localArtifacts(loadEnv(mode, process.cwd(), 'FUJIMOD_')),
    {
      name: 'web-output-directories',
      enforce: 'post',
      config() {
        const output = mode === 'local-preview' ? 'dist-local-preview' : 'dist';
        return {
          build: { outDir: `${output}/client` },
          environments: {
            client: { build: { outDir: `${output}/client` } },
            ssr: { build: { outDir: `${output}/server` } },
          },
        };
      },
    },
  ],
  lint: uiLint,
  fmt: {
    singleQuote: true,
    ignorePatterns: [
      'build/**',
      'dist/**',
      'dist-local-preview/**',
      'public/runtime/**',
      'public/artifacts/**',
    ],
  },
  test: {
    pool: 'forks',
    projects: [
      {
        extends: true,
        test: {
          name: 'unit',
          environment: 'node',
          include: ['src/**/*.test.ts', 'tests/**/*.test.ts'],
        },
      },
      {
        extends: true,
        test: {
          name: 'components',
          environment: 'jsdom',
          include: ['src/**/*.test.tsx'],
        },
      },
    ],
  },
  server: {
    headers: { 'Cache-Control': 'no-store' },
    allowedHosts: ['.ts.net'],
    watch: {
      ignored: [
        '**/build/**',
        `${normalizePath(resolve('dist'))}/**`,
        `${normalizePath(resolve('dist-local-preview'))}/**`,
        '**/browser-bug-reports/**',
      ],
    },
    proxy: { '/usb-proxy': { target: 'http://127.0.0.1:5202', ws: true } },
    host: '127.0.0.1',
    fs: { strict: true, allow: [resolve('../..')] },
  },
  preview: {
    proxy: { '/usb-proxy': { target: 'http://127.0.0.1:5202', ws: true } },
    host: '127.0.0.1',
    allowedHosts: ['.ts.net'],
  },
  build: { target: 'es2022' },
  worker: { format: 'es' },
}));
