const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
const KINDS = ['error', 'rejection', 'console', 'fault'];

/** What the website sends for one error in a visitor's browser. */
export interface ClientErrorRecord {
  id: string;
  /** One per page load; groups every record from the same open page. */
  session: string;
  kind: 'error' | 'rejection' | 'console' | 'fault';
  message: string;
  stack?: string;
  fault?: unknown;
  page: string;
  build: string;
  userAgent: string;
  /** Browser, OS, screen, viewport and hardware details from the website. */
  device: {
    browser?: string;
    os?: string;
    screen?: { width?: number; height?: number } & Record<string, unknown>;
  } & Record<string, unknown>;
  createdAt: string;
  uptime: number;
}

export function parseClientError(text: string): ClientErrorRecord {
  const value = JSON.parse(text);
  const string = (key: string) => typeof value?.[key] === 'string';
  if (
    !value ||
    typeof value !== 'object' ||
    !uuid.test(value.id) ||
    !uuid.test(value.session) ||
    !KINDS.includes(value.kind) ||
    !['message', 'page', 'build', 'userAgent', 'createdAt'].every(string) ||
    (value.stack !== undefined && !string('stack')) ||
    typeof value.uptime !== 'number' ||
    !value.device ||
    typeof value.device !== 'object' ||
    Array.isArray(value.device)
  )
    throw Error('Invalid record');
  return value;
}

/** One Workers Logs entry. `message` reads on its own in the log list; the
 * remaining fields are indexed for filtering. Everything describes the visitor's
 * browser, never this Worker. */
export function clientErrorLog(record: ClientErrorRecord, country: string | undefined) {
  const path = new URL(record.page).pathname;
  const first = record.message.split('\n')[0];
  const { browser = 'unknown browser', os = 'unknown OS', screen } = record.device;
  const size = screen?.width && screen.height ? ` ${screen.width}×${screen.height}` : '';
  return {
    message: `[browser ${record.session.slice(0, 8)} · ${browser} · ${os}${size}] ${record.kind} on ${path}: ${first}`,
    source: 'website-client',
    client: {
      session: record.session,
      page: record.page,
      build: record.build,
      userAgent: record.userAgent,
      device: record.device,
      country: country ?? 'unknown',
      reportedAt: record.createdAt,
      secondsSinceLoad: Math.round(record.uptime / 100) / 10,
    },
    error: {
      id: record.id,
      kind: record.kind,
      message: record.message,
      ...(record.stack ? { stack: record.stack } : {}),
      ...(record.fault ? { fault: record.fault } : {}),
    },
  };
}
