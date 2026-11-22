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
  engine?: 'unicorn' | 'hypervisor';
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
  launchConnection(): Promise<LaunchRequest | undefined>;
  close(): Promise<void>;
}
