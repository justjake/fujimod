import { inflateRawSync } from 'node:zlib';
import { crc32 } from '@fujimod/emulator-ui/archive';
import type { Plugin, Connect } from 'vite';
import { browserScript } from './browser-script.ts';
import { createWriteStream } from 'node:fs';
import { mkdir, open, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve } from 'node:path';
import { Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';

const endpoint = '/__simulator-reports';
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
export function localReports(
  directory: string,
  logger: Pick<Console, 'info'> = console,
): Connect.NextHandleFunction {
  const root = resolve(directory);
  return async (req, res, next) => {
    const path = new URL(req.url ?? '/', 'http://local').pathname;
    if (path !== endpoint && !path.startsWith(endpoint + '/')) return next();
    const reply = (status: number, value: unknown) => {
      res.statusCode = status;
      res.setHeader('Content-Type', 'application/json');
      res.end(JSON.stringify(value));
    };
    try {
      if (req.headers.origin && new URL(req.headers.origin).host !== req.headers.host)
        return reply(403, { error: 'Cross-origin report' });
      if (path === endpoint && req.method === 'POST') {
        let length = 0;
        const chunks = [];
        for await (const chunk of req) {
          length += chunk.length;
          if (length > 1024 * 1024) return reply(413, { error: 'Report metadata too large' });
          chunks.push(chunk);
        }
        let report;
        try {
          report = JSON.parse(Buffer.concat(chunks).toString());
        } catch {
          return reply(400, { error: 'Invalid report JSON' });
        }
        if (
          !uuid.test(report.id) ||
          !['emulator', 'dev-tools'].includes(report.source) ||
          !['feedback', 'crash'].includes(report.event)
        )
          return reply(400, { error: 'Invalid report identity' });
        const folder = resolve(root, report.id);
        await mkdir(root, { recursive: true });
        try {
          await mkdir(folder);
        } catch (error) {
          if (error instanceof Error && 'code' in error && error.code === 'EEXIST')
            return reply(409, { error: 'Report already exists' });
          throw error;
        }
        await writeFile(
          resolve(folder, 'metadata.json'),
          JSON.stringify(
            {
              ...report,
              receivedAt: new Date().toISOString(),
              archive: { status: report.archiveExpected === false ? 'unavailable' : 'pending' },
            },
            null,
            2,
          ) + '\n',
        );
        logger.info(`[simulator-report] ${folder}`);
        return reply(201, { id: report.id });
      }
      const match = path.match(/^\/__simulator-reports\/([^/]+)\/report\.zip$/);
      if (!match || !uuid.test(match[1]) || req.method !== 'PUT')
        return reply(405, { error: 'Unsupported report operation' });
      const folder = resolve(root, match[1]),
        manifestPath = resolve(folder, 'metadata.json');
      let manifest;
      try {
        manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
      } catch {
        return reply(404, { error: 'Unknown report' });
      }
      const temporary = resolve(folder, 'report.zip.part');
      const hash = createHash('sha256');
      let bytes = 0;
      const bound = new Transform({
        transform(chunk, _encoding, done) {
          bytes += chunk.length;
          if (bytes > 512 * 1024 * 1024) return done(Error('Report archive too large'));
          hash.update(chunk);
          done(null, chunk);
        },
      });
      try {
        await pipeline(req, bound, createWriteStream(temporary, { flags: 'wx' }));
        await rename(temporary, resolve(folder, 'report.zip'));
        manifest.archive = { status: 'saved', bytes, sha256: hash.digest('hex') };
        try {
          await extractReport(resolve(folder, 'report.zip'), resolve(folder, 'report.json'));
        } catch (error) {
          manifest.diagnosticsError = String(error);
        }
        await writeFile(manifestPath, JSON.stringify(manifest, null, 2) + '\n');
        logger.info(`[simulator-report] saved ${folder}/report.zip`);
        return reply(201, { id: match[1], bytes });
      } catch (error) {
        await rm(temporary, { force: true });
        manifest.archive = { status: 'failed', error: String(error) };
        await writeFile(manifestPath, JSON.stringify(manifest, null, 2) + '\n');
        throw error;
      }
    } catch (error) {
      next(error);
    }
  };
}

export function simulatorReports(): Plugin {
  return {
    name: 'simulator-reports',
    apply: 'serve',
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        if (req.url !== '/__simulator-reports.js') return next();
        res.setHeader('Content-Type', 'text/javascript');
        res.setHeader('Cache-Control', 'no-store');
        void browserScript(new URL('./report-bootstrap.ts', import.meta.url))
          .then((bytes) => res.end(bytes))
          .catch(next);
      });
      server.middlewares.use(
        localReports(resolve(server.config.root, 'browser-bug-reports'), server.config.logger),
      );
    },
  };
}

async function extractReport(archive: string, destination: string) {
  const file = await open(archive, 'r');
  try {
    const header = Buffer.alloc(30);
    if (
      (await file.read(header, 0, 30, 0)).bytesRead !== 30 ||
      header.readUInt32LE(0) !== 0x04034b50 ||
      ![0, 8].includes(header.readUInt16LE(8))
    )
      throw Error('Report ZIP header differs');
    const size = header.readUInt32LE(18),
      length = header.readUInt16LE(26),
      extra = header.readUInt16LE(28);
    if (size > 16 * 1024 * 1024 || length !== 11) throw Error('Report JSON extent differs');
    const name = Buffer.alloc(length);
    await file.read(name, 0, length, 30);
    if (name.toString() !== 'report.json') throw Error('Report JSON is not the first ZIP member');
    const data = Buffer.alloc(size);
    if ((await file.read(data, 0, size, 30 + length + extra)).bytesRead !== size)
      throw Error('Report JSON is truncated');
    const decoded =
      header.readUInt16LE(8) === 8
        ? inflateRawSync(data, { maxOutputLength: 16 * 1024 * 1024 })
        : data;
    if (decoded.length !== header.readUInt32LE(22) || crc32(decoded) !== header.readUInt32LE(14))
      throw Error('Report JSON checksum differs');
    await writeFile(destination, JSON.stringify(JSON.parse(decoded.toString()), null, 2) + '\n');
  } finally {
    await file.close();
  }
}
