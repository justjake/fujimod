import type { Scene } from '../engine/api';

/** One camera stream and at most one RGB frame awaiting the emulator. */
export class WebcamInput {
  private generation = 0;
  private stream?: MediaStream;
  private video?: HTMLVideoElement;
  private timer?: ReturnType<typeof setTimeout>;
  requested = false;

  constructor(
    private send: (scene: Scene | null) => Promise<void>,
    private changed: (active: boolean, busy: boolean) => void,
    private failed: (error: unknown) => void,
  ) {}

  async start() {
    if (this.requested) return;
    this.requested = true;
    const generation = ++this.generation;
    this.changed(false, true);
    try {
      if (!navigator.mediaDevices?.getUserMedia)
        throw Error('Webcam input is unavailable in this browser.');
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: false,
        video: {
          facingMode: { ideal: 'environment' },
          width: { ideal: 640 },
          height: { ideal: 480 },
        },
      });
      if (generation !== this.generation) {
        for (const track of stream.getTracks()) track.stop();
        return;
      }
      this.stream = stream;
      const video = document.createElement('video');
      this.video = video;
      video.muted = true;
      video.playsInline = true;
      video.srcObject = stream;
      for (const track of stream.getVideoTracks())
        track.addEventListener(
          'ended',
          () => {
            if (generation === this.generation) void this.stop().catch(this.failed);
          },
          { once: true },
        );
      await video.play();
      if (generation !== this.generation) return;
      const canvas = document.createElement('canvas');
      const context = canvas.getContext('2d', { willReadFrequently: true });
      if (!context) throw Error('Webcam frames could not be read.');
      this.changed(true, false);
      const frame = async () => {
        if (generation !== this.generation) return;
        try {
          if (video.readyState >= 2 && video.videoWidth && video.videoHeight) {
            const scale = Math.min(1, 640 / video.videoWidth, 480 / video.videoHeight);
            const width = Math.max(1, Math.round(video.videoWidth * scale));
            const height = Math.max(1, Math.round(video.videoHeight * scale));
            if (canvas.width !== width || canvas.height !== height) {
              canvas.width = width;
              canvas.height = height;
            }
            context.drawImage(video, 0, 0, width, height);
            const rgba = context.getImageData(0, 0, width, height).data;
            const rgb = new Uint8Array(width * height * 3);
            for (let source = 0, target = 0; source < rgba.length; source += 4) {
              rgb[target++] = rgba[source];
              rgb[target++] = rgba[source + 1];
              rgb[target++] = rgba[source + 2];
            }
            await this.send({ width, height, rgb });
          }
          if (generation === this.generation) this.timer = setTimeout(() => void frame(), 100);
        } catch (error) {
          if (generation !== this.generation) return;
          await this.stop().catch(() => {});
          this.failed(error);
        }
      };
      void frame();
    } catch (error) {
      if (generation !== this.generation) return;
      await this.stop().catch(() => {});
      this.failed(error);
    }
  }

  async stop(restore = true) {
    ++this.generation;
    this.requested = false;
    clearTimeout(this.timer);
    this.timer = undefined;
    for (const track of this.stream?.getTracks() ?? []) track.stop();
    this.stream = undefined;
    if (this.video) {
      this.video.pause();
      this.video.srcObject = null;
      this.video = undefined;
    }
    this.changed(false, false);
    if (restore) await this.send(null);
  }
}
