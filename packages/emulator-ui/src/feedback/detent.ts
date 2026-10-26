import { renderDetentSound, type DetentSound } from './detent-sound';

/** Optional, gesture-initiated feedback. Never unlock audio with a media element:
 * WebKit's ambient Web Audio session intentionally obeys the Ring/Silent switch.
 * https://bugs.webkit.org/show_bug.cgi?id=252746
 */
export class DetentFeedback {
  private context?: AudioContext;
  private haptics = false;
  private buffers = new Map<DetentSound, AudioBuffer>();

  beginGesture(event: Event) {
    if (!event.isTrusted) return;
    this.haptics = false;
    try {
      // Reserve tactile feedback for touch; mouse and keyboard get an audible click.
      if ('pointerType' in event && event.pointerType === 'touch') {
        this.haptics = Boolean(navigator.vibrate?.(0));
        if (this.haptics) return;
      }
    } catch {
      /* Permissions or platform restrictions can reject vibration. */
    }
    try {
      // Transient maps to iOS's ambient session. Older WebKit already defaults
      // Web Audio to ambient; leave that default alone when AudioSession is absent.
      const session = (navigator as Navigator & { audioSession?: { type: string } }).audioSession;
      if (session) session.type = 'transient';
      this.context ??= new AudioContext();
      if (this.context.state === 'suspended') void this.context.resume().catch(() => {});
    } catch {
      /* Feedback must never prevent a camera input. */
    }
  }

  step(sound: DetentSound = 'compact') {
    try {
      if (this.haptics && navigator.vibrate?.(8)) return;
    } catch {
      /* Unsupported vibration falls through to an already unlocked context. */
    }
    const context = this.context;
    if (!context || context.state !== 'running' || document.hidden) return;
    let buffer = this.buffers.get(sound);
    if (!buffer) {
      const samples = renderDetentSound(sound, context.sampleRate);
      buffer = context.createBuffer(1, samples.length, context.sampleRate);
      buffer.copyToChannel(samples, 0);
      this.buffers.set(sound, buffer);
    }
    const source = context.createBufferSource();
    source.buffer = buffer;
    // Tiny variations keep repeated ticks from sounding like a sampled loop.
    source.playbackRate.value = 0.985 + Math.random() * 0.03;
    source.connect(context.destination);
    source.onended = () => source.disconnect();
    source.start();
  }
}
