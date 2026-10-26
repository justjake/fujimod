import { expect, test } from 'vite-plus/test';
import { bugReportRoute } from './route.ts';
test('the QR path redirects to the private entry without losing its fragment', () => {
  let middleware: Function;
  bugReportRoute().configureServer({
    middlewares: {
      use(value: Function) {
        middleware = value;
      },
    },
  } as unknown as import('vite').ViteDevServer);
  const headers: Record<string, string> = {},
    response = {
      statusCode: 0,
      setHeader(name: string, value: string) {
        headers[name] = value;
      },
      end() {},
    };
  middleware!({ url: '/bug?test=1' }, response, () => {
    throw Error('unexpected fallback');
  });
  expect(response.statusCode).toBe(307);
  expect(headers.Location).toBe('/bug/?test=1');
});
