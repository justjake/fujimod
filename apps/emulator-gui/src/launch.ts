export type RunEngine = 'unicorn' | 'hypervisor';
export interface LaunchRequest {
  /** Catalog body identity. */
  body: string;
  /** A version, a firmware identity or `none`; the body's newest when omitted. */
  firmware?: string;
  update: string;
  engine?: RunEngine;
  endpoint?: string;
}
export type BootChoice = 'stock' | 'sd-card' | 'autostart';
export interface LauncherCamera {
  firmware: string;
  body: string;
  model: string;
  version: string;
}
export interface OpenCameraRequest {
  body: string;
  firmware: string;
  boot: BootChoice;
  sdCard?: string;
  skipBootCache?: boolean;
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
  chooseUpdate(): Promise<string | undefined>;
  launch(request: LaunchRequest): Promise<LaunchedCamera>;
  launcherCameras(): Promise<LauncherCamera[]>;
  chooseSdCard(): Promise<string | undefined>;
  openCamera(request: OpenCameraRequest): Promise<LaunchedCamera>;
  connect(endpoint: string): Promise<LaunchedCamera>;
  launchConnection(): Promise<LaunchRequest | undefined>;
  connection(): Promise<string | undefined>;
  close(): Promise<void>;
}
