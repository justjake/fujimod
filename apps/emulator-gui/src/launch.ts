export interface RunConfiguration {
  configuration: string;
  body: string;
  model: string;
  firmware: string;
  updateSha256: string;
  updateBytes: number;
}

export interface LaunchRequest {
  configuration: string;
  update: string;
}

export interface LaunchedCamera {
  id: string;
  configuration: string;
  body: string;
  model: string;
  firmware: string;
  endpoint: string;
  standalone: boolean;
}

export interface DesktopBridge {
  configurations(): Promise<RunConfiguration[]>;
  chooseUpdate(): Promise<string | undefined>;
  launch(request: LaunchRequest): Promise<LaunchedCamera>;
  launchCommand(): Promise<LaunchedCamera | undefined>;
  launchCli(): Promise<LaunchedCamera | undefined>;
  close(): Promise<void>;
}

/** The CLI passes a live endpoint and file choice through the window URL. */
export function commandLineLaunch(): LaunchedCamera | undefined {
  const url = new URL(location.href);
  const endpoint = url.searchParams.get('endpoint');
  if (!endpoint) return undefined;
  return {
    id: url.searchParams.get('id') ?? endpoint,
    configuration: url.searchParams.get('configuration') ?? 'Native emulator',
    body: url.searchParams.get('body') ?? '',
    model: url.searchParams.get('model') ?? 'FujiMod Emulator',
    firmware: url.searchParams.get('firmware') ?? '',
    endpoint,
    standalone: false,
  };
}

/** A launcher URL carries the same fields as a CLI-opened window. */
export const desktopLaunch = commandLineLaunch;
