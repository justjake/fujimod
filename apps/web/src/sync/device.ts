/** Device authorization against the FujiMod Worker (RFC 8628).
 *
 * The camera shows the user code, polls for a session token once the owner
 * approves it, and trades that token for the owner's Google access token. */

export interface DeviceCode {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  verificationUriComplete?: string;
  expiresIn: number;
  interval: number;
}

export interface DeviceRequest {
  serverUrl: string;
  clientId: string;
  scope: string;
  fetch: typeof fetch;
}

export interface GoogleToken {
  accessToken: string;
  expiresAt?: string;
}

export class DeviceAuthorizationError extends Error {
  constructor(
    message: string,
    readonly code?: string,
  ) {
    super(message);
    this.name = 'DeviceAuthorizationError';
  }
}

interface ErrorBody {
  error?: string | { code?: string; message?: string };
  error_description?: string;
  message?: string;
}

function errorOf(body: ErrorBody | undefined, status: number): DeviceAuthorizationError {
  const code = typeof body?.error === 'string' ? body.error : body?.error?.code;
  const message =
    body?.error_description ??
    (typeof body?.error === 'object' ? body.error.message : undefined) ??
    body?.message ??
    `The device request failed with ${status}.`;
  return new DeviceAuthorizationError(message, code);
}

export async function requestDeviceCode(request: DeviceRequest): Promise<DeviceCode> {
  const response = await request.fetch(`${request.serverUrl}/api/auth/device/code`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ client_id: request.clientId, scope: request.scope }),
  });
  const body = (await response.json().catch(() => undefined)) as
    | (ErrorBody & {
        device_code?: string;
        user_code?: string;
        verification_uri?: string;
        verification_uri_complete?: string;
        expires_in?: number;
        interval?: number;
      })
    | undefined;
  if (!response.ok) throw errorOf(body, response.status);
  if (!body?.device_code || !body.user_code || !body.verification_uri)
    throw new DeviceAuthorizationError('The Worker returned an incomplete device code.');
  return {
    deviceCode: body.device_code,
    userCode: body.user_code,
    verificationUri: body.verification_uri,
    verificationUriComplete: body.verification_uri_complete,
    expiresIn: body.expires_in ?? 600,
    interval: body.interval ?? 5,
  };
}

/** Polls until the owner approves, then returns the camera's session token. */
export async function pollDeviceToken(
  request: DeviceRequest,
  code: DeviceCode,
  options: { sleep?: (milliseconds: number) => Promise<void>; now?: () => number } = {},
): Promise<string> {
  const sleep =
    options.sleep ??
    ((milliseconds: number) => new Promise<void>((resolve) => setTimeout(resolve, milliseconds)));
  const now = options.now ?? Date.now;
  const deadline = now() + code.expiresIn * 1000;
  let interval = code.interval;
  for (;;) {
    const response = await request.fetch(`${request.serverUrl}/api/auth/device/token`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
        device_code: code.deviceCode,
        client_id: request.clientId,
      }),
    });
    const body = (await response.json().catch(() => undefined)) as
      | (ErrorBody & { access_token?: string })
      | undefined;
    if (response.ok && body?.access_token) return body.access_token;
    const error = errorOf(body, response.status);
    if (error.code === 'authorization_pending') {
      if (now() + interval * 1000 > deadline) throw expired();
      await sleep(interval * 1000);
      continue;
    }
    if (error.code === 'slow_down') {
      interval += 5;
      if (now() + interval * 1000 > deadline) throw expired();
      await sleep(interval * 1000);
      continue;
    }
    throw error;
  }
}

function expired(): DeviceAuthorizationError {
  return new DeviceAuthorizationError(
    'The device code expired before it was approved.',
    'expired_token',
  );
}

/** Exchanges the camera's session token for the owner's Google access token. */
export async function googleAccessToken(request: {
  serverUrl: string;
  deviceToken: string;
  fetch: typeof fetch;
}): Promise<GoogleToken> {
  const response = await request.fetch(`${request.serverUrl}/api/devices/google-token`, {
    method: 'POST',
    headers: { authorization: `Bearer ${request.deviceToken}` },
  });
  const body = (await response.json().catch(() => undefined)) as
    | (ErrorBody & { accessToken?: string; expiresAt?: string | null })
    | undefined;
  if (!response.ok || !body?.accessToken) throw errorOf(body, response.status);
  return { accessToken: body.accessToken, expiresAt: body.expiresAt ?? undefined };
}
