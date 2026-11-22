export interface RunConfiguration {
  configuration: string;
  body: string;
  model: string;
  firmware: string;
  updateSha256: string;
  updateBytes: number;
}
export type RunEngine = 'unicorn' | 'hypervisor';
export interface LaunchRequest {
  configuration: string;
  update: string;
  engine?: RunEngine;
  endpoint?: string;
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
  connect(endpoint: string): Promise<LaunchedCamera>;
  launchConnection(): Promise<LaunchRequest | undefined>;
  connection(): Promise<string | undefined>;
  close(): Promise<void>;
}
