import type { D1Database } from '@cloudflare/workers-types';
import type { DevicePermission, DevicePolicy } from './policy.ts';

export interface ApprovedCode {
  userId: string;
  clientId: string;
}

export interface DeviceGrant {
  deviceId: string;
  userId: string;
  permissions: DevicePermission[];
}

/** A device credential is the session token the camera was issued, stored as a hash. */
export async function tokenHash(token: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(token));
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

/** The approved code a token request is about to redeem, read before it is consumed. */
export async function approvedCode(
  db: D1Database,
  deviceCode: string,
): Promise<ApprovedCode | undefined> {
  const row = await db
    .prepare(
      `select user_id as userId, client_id as clientId from device_code
       where device_code = ? and status = 'approved' and user_id is not null`,
    )
    .bind(deviceCode)
    .first<{ userId: string | null; clientId: string | null }>();
  if (!row?.userId || !row.clientId) return undefined;
  return { userId: row.userId, clientId: row.clientId };
}

/** Remembers the device a session token belongs to, so later requests can be
 * checked against the permissions this client is allowed to hold. */
export async function recordCredential(
  db: D1Database,
  record: { userId: string; token: string; policy: DevicePolicy },
): Promise<void> {
  const hash = await tokenHash(record.token);
  const now = Date.now();
  const existing = await db
    .prepare(
      `select id from device where user_id = ? and name = ? and kind = ? and status = 'active'`,
    )
    .bind(record.userId, record.policy.name, record.policy.kind)
    .first<{ id: string }>();
  const deviceId = existing?.id ?? crypto.randomUUID();
  const device = existing
    ? db.prepare('update device set last_seen_at = ? where id = ?').bind(now, deviceId)
    : db
        .prepare(
          'insert into device (id, user_id, name, kind, status, created_at) values (?, ?, ?, ?, ?, ?)',
        )
        .bind(deviceId, record.userId, record.policy.name, record.policy.kind, 'active', now);
  await db.batch([
    device,
    ...record.policy.permissions.map((permission) =>
      db
        .prepare('insert or ignore into device_permission (device_id, permission) values (?, ?)')
        .bind(deviceId, permission),
    ),
    db
      .prepare(
        'insert or ignore into device_credential (id, device_id, token_hash, created_at) values (?, ?, ?, ?)',
      )
      .bind(crypto.randomUUID(), deviceId, hash, now),
  ]);
}

/** The live device behind a bearer token, with the permissions it holds. */
export async function credentialGrant(
  db: D1Database,
  token: string,
): Promise<DeviceGrant | undefined> {
  const row = await db
    .prepare(
      `select device.id as deviceId, device.user_id as userId from device_credential
       join device on device.id = device_credential.device_id
       where device_credential.token_hash = ? and device_credential.revoked_at is null
         and device.status = 'active' and device.revoked_at is null`,
    )
    .bind(await tokenHash(token))
    .first<{ deviceId: string; userId: string }>();
  if (!row) return undefined;
  const permissions = await db
    .prepare('select permission from device_permission where device_id = ?')
    .bind(row.deviceId)
    .all<{ permission: DevicePermission }>();
  return {
    deviceId: row.deviceId,
    userId: row.userId,
    permissions: permissions.results.map((entry) => entry.permission),
  };
}

export async function touchCredential(db: D1Database, token: string): Promise<void> {
  await db
    .prepare('update device_credential set last_used_at = ? where token_hash = ?')
    .bind(Date.now(), await tokenHash(token))
    .run();
}

/** The user's linked Google account, which holds the refreshable OAuth token. */
export async function googleAccountId(db: D1Database, userId: string): Promise<string | undefined> {
  const row = await db
    .prepare("select id from account where user_id = ? and provider_id = 'google'")
    .bind(userId)
    .first<{ id: string }>();
  return row?.id;
}
