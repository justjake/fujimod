/** Browser calls to the Worker's Better Auth routes. */

export interface SessionUser {
  id: string;
  email: string;
  name: string;
}

export type DeviceStatus = 'pending' | 'approved' | 'denied';

export interface DeviceRequest {
  user_code: string;
  status: DeviceStatus;
  client_id?: string;
  scope?: string;
}

const base = '/api/auth';

function call(path: string, init: RequestInit = {}): Promise<Response> {
  return fetch(`${base}${path}`, { credentials: 'same-origin', ...init });
}

function post(path: string, body: unknown): Promise<Response> {
  return call(path, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
}

/** Better Auth reports failures as `{error, error_description}`. */
async function failure(response: Response, action: string): Promise<Error> {
  const body = (await response.json().catch(() => undefined)) as
    | { error?: string; error_description?: string }
    | undefined;
  return new Error(body?.error_description || body?.error || `${action}: HTTP ${response.status}`);
}

export async function sessionUser(): Promise<SessionUser | null> {
  const response = await call('/get-session');
  if (!response.ok) return null;
  const body = (await response.json().catch(() => null)) as { user?: SessionUser } | null;
  return body?.user ?? null;
}

/** Starts Google sign-in and returns the provider URL to visit. */
export async function googleSignInUrl(callbackURL: string): Promise<string> {
  const response = await post('/sign-in/social', { provider: 'google', callbackURL });
  if (!response.ok) throw await failure(response, 'Sign-in failed');
  const { url } = (await response.json()) as { url?: string };
  if (!url) throw new Error('Sign-in returned no URL');
  return url;
}

/** Claims a pending code for the signed-in user and reads its status. */
export async function deviceRequest(userCode: string): Promise<DeviceRequest> {
  const response = await call(`/device?user_code=${encodeURIComponent(userCode)}`);
  if (!response.ok) throw await failure(response, 'Device code failed');
  return (await response.json()) as DeviceRequest;
}

export async function resolveDevice(userCode: string, approve: boolean): Promise<void> {
  const response = await post(approve ? '/device/approve' : '/device/deny', { userCode });
  if (!response.ok) throw await failure(response, 'Device approval failed');
}
