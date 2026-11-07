import { expect, test } from 'vite-plus/test';
import { directoryRoute } from './directory-route.ts';

function request(plugin: ReturnType<typeof directoryRoute>, url: string) {
  let middleware: Function;
  plugin.configureServer({
    middlewares: {
      use(value: Function) {
        middleware = value;
      },
    },
  } as unknown as import('vite').ViteDevServer);
  const headers: Record<string, string> = {};
  const response = {
    statusCode: 0,
    setHeader(name: string, value: string) {
      headers[name] = value;
    },
    end() {},
  };
  let fellThrough = false;
  middleware!({ url }, response, () => {
    fellThrough = true;
  });
  return { response, headers, fellThrough };
}

test('a bare directory path redirects to its entry point with the query intact', () => {
  const { response, headers } = request(directoryRoute('bug'), '/bug?test=1');
  expect(response.statusCode).toBe(307);
  expect(headers.Location).toBe('/bug/?test=1');
});

test('a device code link keeps its code when the path loses its slash', () => {
  const { response, headers } = request(directoryRoute('device'), '/device?user_code=2GK5QTNV');
  expect(response.statusCode).toBe(307);
  expect(headers.Location).toBe('/device/?user_code=2GK5QTNV');
});

test('other paths fall through', () => {
  expect(request(directoryRoute('device'), '/device-files/').fellThrough).toBe(true);
  expect(request(directoryRoute('device'), '/device/').fellThrough).toBe(true);
});
