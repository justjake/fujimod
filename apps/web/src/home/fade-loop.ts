/** Fade duration, then a held black frame, between the end of a preview and its restart. */
export const LOOP_FADE_MS = 350;
export const LOOP_PAUSE_MS = 250;

/**
 * Loops a muted preview video by fading it out, pausing, and fading it back in
 * from its first frame. `data-fade` drives the CSS transition. Call `restart`
 * to loop early (a clip range); returns a disposer.
 */
export function fadeLoop(
  video: HTMLVideoElement,
  start: () => number = () => 0,
): { restart: () => void; dispose: () => void } {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let restarting = false;
  const resume = () => {
    restarting = false;
    delete video.dataset.fade;
    void video.play().catch(() => {});
  };
  const restart = () => {
    if (restarting) return;
    restarting = true;
    video.dataset.fade = '';
    timer = setTimeout(() => {
      video.pause();
      video.addEventListener('seeked', resume, { once: true });
      video.currentTime = start();
    }, LOOP_FADE_MS + LOOP_PAUSE_MS);
  };
  video.addEventListener('ended', restart);
  return {
    restart,
    dispose: () => {
      clearTimeout(timer);
      video.removeEventListener('ended', restart);
      video.removeEventListener('seeked', resume);
    },
  };
}
