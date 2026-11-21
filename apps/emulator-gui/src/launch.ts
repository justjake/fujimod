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
}

export interface DesktopBridge {
  configurations(): Promise<RunConfiguration[]>;
  chooseUpdate(): Promise<string | undefined>;
  launch(request: LaunchRequest): Promise<LaunchedCamera>;
  close(): Promise<void>;
}

declare global {
  interface Window {
    fujimodLaunch?: LaunchedCamera;
  }
}

/** A CLI-opened window receives its live endpoint before the page starts. */
export function commandLineLaunch(): LaunchedCamera | undefined {
  const launch = window.fujimodLaunch;
  return launch?.endpoint ? launch : undefined;
}
