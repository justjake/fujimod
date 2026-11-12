import { Show, createEffect, createMemo, createSignal } from 'solid-js';
import type { Camera, ShareOffline } from './camera-model';

/** Two rising tones. Browsers allow audio only after a click, so `unlock` runs inside one. */
export function createChime() {
  let audio: AudioContext | undefined;
  return {
    unlock() {
      if (typeof AudioContext === 'undefined') return;
      audio ??= new AudioContext();
      void audio.resume();
    },
    play() {
      if (!audio || audio.state !== 'running') return;
      const now = audio.currentTime;
      for (const [offset, frequency] of [
        [0, 880],
        [0.18, 1175],
      ]) {
        const tone = audio.createOscillator();
        const gain = audio.createGain();
        tone.frequency.value = frequency;
        gain.gain.setValueAtTime(0.0001, now + offset);
        gain.gain.exponentialRampToValueAtTime(0.25, now + offset + 0.02);
        gain.gain.exponentialRampToValueAtTime(0.0001, now + offset + 0.35);
        tone.connect(gain).connect(audio.destination);
        tone.start(now + offset);
        tone.stop(now + offset + 0.4);
      }
    },
  };
}
export type Chime = ReturnType<typeof createChime>;

/** The page's chime, unlocked by Connect camera, Sharing On and Reconnect camera. */
export const ownerChime = createChime();

function message(camera: Camera, reason: ShareOffline) {
  if (reason === 'battery')
    return `${camera.name} battery ran out. Charge it and turn it on, then reconnect it to keep sharing on the same link.`;
  if (reason === 'different camera')
    return 'This is a different camera. Connect the camera that was shared.';
  return `${camera.name} disconnected. Turn it on and reconnect it to keep sharing on the same link.`;
}

/**
 * A shared camera went away: one modal, a chime and a shake on arrival, and
 * Reconnect camera. It closes itself when the camera is shared again; Close
 * dismisses it until the next disconnect. An exhausted battery is expected and
 * does not chime.
 */
export function ShareAlert(props: {
  cameras: readonly Camera[];
  chime: Chime;
  connecting: boolean;
  onReconnect: () => void;
}) {
  let dialog!: HTMLDialogElement;
  const [dismissed, setDismissed] = createSignal<ReadonlySet<string>>(new Set());
  const incidents = createMemo(() =>
    props.cameras.flatMap((camera) =>
      camera.sharing?.offline
        ? [
            {
              key: `${camera.id}:${camera.sharing.offline}`,
              camera,
              reason: camera.sharing.offline,
            },
          ]
        : [],
    ),
  );
  const shown = createMemo(() => incidents().find(({ key }) => !dismissed().has(key)));
  createEffect(
    () => incidents().map(({ key }) => key),
    (keys) => {
      setDismissed((set) => {
        const kept = [...set].filter((key) => keys.includes(key));
        return kept.length === set.size ? set : new Set(kept);
      });
    },
  );
  createEffect(
    () => shown(),
    (incident, previous) => {
      if (!dialog) return;
      if (!incident) {
        if (dialog.open) dialog.close();
        return;
      }
      if (!dialog.open) dialog.showModal();
      if (incident.key === previous?.key) return;
      if (incident.reason !== 'battery') {
        props.chime.play();
        dialog.classList.remove('shake');
        void dialog.offsetWidth; // restart the animation for each incident
        dialog.classList.add('shake');
      }
    },
  );
  const dismiss = () => {
    const key = shown()?.key;
    if (key) setDismissed((set) => new Set(set).add(key));
  };
  return (
    <dialog
      ref={(element) => (dialog = element)}
      class="wb-share-alert"
      aria-label="Shared camera disconnected"
      onCancel={(event) => {
        event.preventDefault();
        dismiss();
      }}
    >
      <Show when={shown()}>
        {(incident) => (
          <>
            <p>{message(incident().camera, incident().reason)}</p>
            <div class="wb-share-alert-actions">
              <Show when={!incident().camera.present}>
                <button
                  class="wb-btn wb-primary"
                  disabled={props.connecting}
                  onClick={() => {
                    props.chime.unlock();
                    props.onReconnect();
                  }}
                >
                  {props.connecting ? 'Connecting…' : 'Reconnect camera'}
                </button>
              </Show>
              <button class="wb-btn" onClick={dismiss}>
                Close
              </button>
            </div>
          </>
        )}
      </Show>
    </dialog>
  );
}
