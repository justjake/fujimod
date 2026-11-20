import type { EngineFault } from '@fujimod/emulator-ui/engine/fault';
import { sharingUrl } from './sharing-server';

declare const __FUJIMOD_SIMULATOR_BUILD__: string;

export const ERROR_LOG_URL = sharingUrl('/error-logs');
const SITES = new Set(['fujimod.com', 'www.fujimod.com']);
const LIMIT = 20;
const TEXT = 4000;

export type ErrorLogKind = 'error' | 'rejection' | 'console' | 'fault';
export interface ErrorLogRecord {
  id: string;
  /** One per page load; groups every record from the same open page. */
  session: string;
  kind: ErrorLogKind;
  message: string;
  stack?: string;
  /** The error's own properties (such as a USB error's `kind` and limits) and its `cause` chain. */
  details?: unknown;
  fault?: EngineFault;
  /** Origin and path only: queries and fragments can carry share links. */
  page: string;
  build: string;
  userAgent: string;
  device: DeviceInfo | { unavailable: string };
  createdAt: string;
  /** Milliseconds since the page started loading. */
  uptime: number;
}

/** The visitor's browser and hardware, read when the error happens. */
export interface DeviceInfo {
  browser: string;
  os: string;
  mobile: boolean;
  screen: {
    width: number;
    height: number;
    pixelRatio: number;
    orientation?: string;
  };
  viewport: { width: number; height: number };
  touchPoints: number;
  cores?: number;
  /** Approximate GiB; Chromium only. */
  memory?: number;
  language: string;
  timeZone: string;
  colorScheme: 'dark' | 'light';
  /** Installed to the home screen. */
  standalone: boolean;
  connection?: string;
  crossOriginIsolated: boolean;
  webUsb: boolean;
  webGpu: boolean;
}

interface NavigatorExtras {
  userAgentData?: {
    brands: { brand: string; version: string }[];
    mobile: boolean;
    platform: string;
  };
  deviceMemory?: number;
  connection?: { effectiveType?: string; saveData?: boolean };
}

/** Browser name and version from the user agent. Safari on iOS freezes the OS
 * version in its user agent, so the Safari version is the useful number there. */
export function browserFromUserAgent(ua: string) {
  const match = (pattern: RegExp) => ua.match(pattern)?.[1];
  const version = match(/Version\/([\d.]+)/);
  const browser = / Brave\b/.test(ua)
    ? `Brave (WebKit ${version ?? '?'})`
    : match(/Edg(?:A|iOS)?\/([\d.]+)/)
      ? `Edge ${match(/Edg(?:A|iOS)?\/(\d+)/)}`
      : match(/(?:Firefox|FxiOS)\/([\d.]+)/)
        ? `Firefox ${match(/(?:Firefox|FxiOS)\/(\d+)/)}`
        : match(/CriOS\/(\d+)/)
          ? `Chrome iOS ${match(/CriOS\/(\d+)/)}`
          : match(/Chrome\/(\d+)/)
            ? `Chrome ${match(/Chrome\/(\d+)/)}`
            : version && /Safari\//.test(ua)
              ? `Safari ${version}`
              : 'unknown';
  const os = /iPhone/.test(ua)
    ? 'iOS'
    : /iPad/.test(ua)
      ? 'iPadOS'
      : /Android/.test(ua)
        ? `Android ${match(/Android ([\d.]+)/) ?? ''}`.trim()
        : /Mac OS X/.test(ua)
          ? 'macOS'
          : /Windows/.test(ua)
            ? 'Windows'
            : /CrOS/.test(ua)
              ? 'ChromeOS'
              : /Linux/.test(ua)
                ? 'Linux'
                : 'unknown';
  return { browser, os };
}

const media = (query: string) => typeof matchMedia === 'function' && matchMedia(query).matches;

export function deviceInfo(): DeviceInfo {
  const extras = navigator as Navigator & NavigatorExtras;
  const parsed = browserFromUserAgent(navigator.userAgent);
  // Chromium names the actual product (Brave, Edge, Opera) only in its brand list.
  const brand = extras.userAgentData?.brands.find(({ brand }) => !/Not|Chromium/.test(brand));
  if (brand) parsed.browser = `${brand.brand} ${brand.version}`;
  // iPadOS Safari reports itself as a Mac; touch support tells them apart.
  const os = parsed.os === 'macOS' && navigator.maxTouchPoints > 1 ? 'iPadOS' : parsed.os;
  return {
    ...parsed,
    os,
    mobile: extras.userAgentData?.mobile ?? /Mobile|Android/.test(navigator.userAgent),
    screen: {
      width: screen.width,
      height: screen.height,
      pixelRatio: devicePixelRatio,
      orientation: screen.orientation?.type,
    },
    viewport: { width: innerWidth, height: innerHeight },
    touchPoints: navigator.maxTouchPoints,
    cores: navigator.hardwareConcurrency || undefined,
    memory: extras.deviceMemory,
    language: navigator.language,
    timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    colorScheme: media('(prefers-color-scheme: dark)') ? 'dark' : 'light',
    standalone: media('(display-mode: standalone)'),
    connection: extras.connection?.effectiveType,
    crossOriginIsolated: globalThis.crossOriginIsolated === true,
    webUsb: 'usb' in navigator,
    webGpu: 'gpu' in navigator,
  };
}

function safeDeviceInfo(): DeviceInfo | { unavailable: string } {
  try {
    return deviceInfo();
  } catch (error) {
    return { unavailable: String(error) };
  }
}

const clip = (text: string) => (text.length > TEXT ? `${text.slice(0, TEXT)}…` : text);
interface Description {
  message: string;
  stack?: string;
  details?: unknown;
}
/** Own properties other than message and stack, then the cause chain. */
function errorDetails(error: Error, depth = 0): Record<string, unknown> | undefined {
  const details: Record<string, unknown> = {};
  for (const key of Object.keys(error)) {
    const value = (error as unknown as Record<string, unknown>)[key];
    if (key !== 'cause' && typeof value !== 'function') details[key] = value;
  }
  if (error.cause !== undefined)
    details.cause =
      error.cause instanceof Error && depth < 3
        ? {
            message: `${error.cause.name}: ${error.cause.message}`,
            stack: error.cause.stack,
            ...errorDetails(error.cause, depth + 1),
          }
        : describe(error.cause).message;
  return Object.keys(details).length ? details : undefined;
}
function describe(value: unknown): Description {
  if (value instanceof Error)
    return {
      message: clip(`${value.name}: ${value.message}`),
      stack: value.stack && clip(value.stack),
      details: errorDetails(value),
    };
  if (typeof value === 'string') return { message: clip(value) };
  try {
    return { message: clip(JSON.stringify(value) ?? String(value)) };
  } catch {
    return { message: clip(String(value)) };
  }
}
/** Console arguments as one record: every argument in the message, and the first
 * error's stack and details. */
function describeConsole(values: unknown[]): Description {
  const error = values.find((value) => value instanceof Error);
  const message = clip(values.map((value) => describe(value).message).join(' '));
  return error ? { ...describe(error), message } : { message };
}

/** JSON-safe details within the text limit; anything else becomes clipped text. */
function bounded(details: unknown) {
  try {
    const text = JSON.stringify(details);
    return text.length > TEXT ? { text: clip(text) } : details;
  } catch {
    return { text: clip(String(details)) };
  }
}

/** Sends uncaught errors, console errors and emulator faults from the public site to
 * the error-log intake. Installed before the app hydrates so hydration failures are kept. */
export function installErrorLog(send: (record: ErrorLogRecord) => void = post) {
  const seen = new Set<string>();
  const session = crypto.randomUUID();
  let sent = 0;
  function log(kind: ErrorLogKind, description: Description, fault?: EngineFault) {
    // Logging must never become a page error of its own.
    try {
      record(kind, description, fault);
    } catch {
      /* Drop this record. */
    }
  }
  function record(
    kind: ErrorLogKind,
    { message, stack, details }: Description,
    fault?: EngineFault,
  ) {
    const key = `${kind}\n${message}`;
    if (sent >= LIMIT || seen.has(key)) return;
    seen.add(key);
    sent++;
    // The intake refuses records over 16 KiB; oversized details become text.
    if (fault)
      fault = {
        ...fault,
        details: bounded(fault.details) as EngineFault['details'],
      };
    send({
      id: crypto.randomUUID(),
      session,
      kind,
      message,
      stack,
      details: details === undefined ? undefined : bounded(details),
      fault,
      page: location.origin + location.pathname,
      build: __FUJIMOD_SIMULATOR_BUILD__,
      userAgent: navigator.userAgent,
      device: safeDeviceInfo(),
      createdAt: new Date().toISOString(),
      uptime: Math.round(performance.now()),
    });
  }
  addEventListener('error', (event) => log('error', describe(event.error ?? event.message)));
  addEventListener('unhandledrejection', (event) => log('rejection', describe(event.reason)));
  addEventListener('fujimod-fault', (event) => {
    const fault = (event as CustomEvent<EngineFault>).detail;
    const cause = [fault.details.type, fault.details.error].filter(Boolean).join(': ');
    log('fault', describe(cause ? `${fault.message} ${cause}` : fault.message), fault);
  });
  const error = console.error.bind(console);
  console.error = (...values: unknown[]) => {
    error(...values);
    log('console', describeConsole(values));
  };
}

function post(record: ErrorLogRecord) {
  // text/plain keeps this a simple CORS request; keepalive survives page unloads.
  void fetch(ERROR_LOG_URL, {
    method: 'POST',
    mode: 'cors',
    keepalive: true,
    headers: { 'Content-Type': 'text/plain' },
    body: JSON.stringify(record),
  }).catch(() => {});
}

if (typeof window !== 'undefined' && SITES.has(location.hostname)) installErrorLog();
