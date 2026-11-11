import { expect, test } from 'vite-plus/test';
import {
  DeviceAuthorizationError,
  googleAccessToken,
  pollDeviceToken,
  requestDeviceCode,
} from './device.ts';

const request = {
  serverUrl: 'https://fujimod.example',
  clientId: 'x70-camera',
  scope: 'google.photos.append',
};

function scripted(...responses: Response[]) {
  const calls: { url: string; init: RequestInit }[] = [];
  const fetch: typeof globalThis.fetch = async (
    input: string | URL | Request,
    init?: RequestInit,
  ) => {
    calls.push({ url: String(input), init: init ?? {} });
    const response = responses.shift();
    if (!response) throw new Error('The client sent an unexpected request.');
    return response;
  };
  return { calls, fetch };
}

test('a device code carries the user code and polling interval', async () => {
  const { calls, fetch } = scripted(
    Response.json({
      device_code: 'device-1',
      user_code: 'ABCD-EFGH',
      verification_uri: 'https://fujimod.example/device',
      verification_uri_complete: 'https://fujimod.example/device?user_code=ABCD-EFGH',
      expires_in: 900,
      interval: 5,
    }),
  );
  const code = await requestDeviceCode({ ...request, fetch });
  expect(code).toEqual({
    deviceCode: 'device-1',
    userCode: 'ABCD-EFGH',
    verificationUri: 'https://fujimod.example/device',
    verificationUriComplete: 'https://fujimod.example/device?user_code=ABCD-EFGH',
    expiresIn: 900,
    interval: 5,
  });
  expect(calls[0]!.url).toBe('https://fujimod.example/api/auth/device/code');
  expect(JSON.parse(String(calls[0]!.init.body))).toEqual({
    client_id: 'x70-camera',
    scope: 'google.photos.append',
  });
});

test('polling waits while the code is pending and stops at the token', async () => {
  const { calls, fetch } = scripted(
    Response.json({ error: 'authorization_pending' }, { status: 400 }),
    Response.json({ access_token: 'camera-token', token_type: 'Bearer' }),
  );
  const waits: number[] = [];
  const token = await pollDeviceToken(
    { ...request, fetch },
    {
      deviceCode: 'device-1',
      userCode: 'ABCD-EFGH',
      verificationUri: 'https://fujimod.example/device',
      expiresIn: 900,
      interval: 5,
    },
    { sleep: async (milliseconds) => void waits.push(milliseconds) },
  );
  expect(token).toBe('camera-token');
  expect(waits).toEqual([5000]);
  expect(JSON.parse(String(calls[0]!.init.body))).toMatchObject({
    grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
    device_code: 'device-1',
    client_id: 'x70-camera',
  });
});

test('slow_down widens the poll interval', async () => {
  const { fetch } = scripted(
    Response.json({ error: 'slow_down' }, { status: 400 }),
    Response.json({ access_token: 'camera-token' }),
  );
  const waits: number[] = [];
  await pollDeviceToken(
    { ...request, fetch },
    {
      deviceCode: 'device-1',
      userCode: 'ABCD-EFGH',
      verificationUri: 'https://fujimod.example/device',
      expiresIn: 900,
      interval: 5,
    },
    { sleep: async (milliseconds) => void waits.push(milliseconds) },
  );
  expect(waits).toEqual([10_000]);
});

test('a denied code is reported and is not retried', async () => {
  const { calls, fetch } = scripted(Response.json({ error: 'access_denied' }, { status: 400 }));
  const failure = pollDeviceToken(
    { ...request, fetch },
    {
      deviceCode: 'device-1',
      userCode: 'ABCD-EFGH',
      verificationUri: 'https://fujimod.example/device',
      expiresIn: 900,
      interval: 5,
    },
    { sleep: async () => {} },
  );
  await expect(failure).rejects.toBeInstanceOf(DeviceAuthorizationError);
  await expect(failure).rejects.toMatchObject({ code: 'access_denied' });
  expect(calls).toHaveLength(1);
});

test('the Google token is requested with the camera bearer token', async () => {
  const { calls, fetch } = scripted(
    Response.json({ accessToken: 'google-token', expiresAt: '2030-01-01T00:00:00.000Z' }),
  );
  const token = await googleAccessToken({
    serverUrl: 'https://fujimod.example',
    deviceToken: 'camera-token',
    fetch,
  });
  expect(token).toEqual({
    accessToken: 'google-token',
    expiresAt: '2030-01-01T00:00:00.000Z',
  });
  expect(new Headers(calls[0]!.init.headers).get('authorization')).toBe('Bearer camera-token');
});

test('a camera without a linked Google account is refused', async () => {
  const { fetch } = scripted(
    Response.json({ error: 'google_account_unavailable' }, { status: 409 }),
  );
  await expect(
    googleAccessToken({ serverUrl: 'https://fujimod.example', deviceToken: 'camera-token', fetch }),
  ).rejects.toMatchObject({ code: 'google_account_unavailable' });
});
