export interface CameraPerformance {
  guestTicks: number;
  virtualSeconds: number;
  wallSeconds: number;
  speed: number;
  averageSpeed: number;
  runtime?: Record<string, unknown>;
}
declare global {
  interface Window {
    cameraPerformance?: CameraPerformance;
    profileCamera?: (limit?: number) => Promise<unknown>;
  }
}
