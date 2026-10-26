import { For, Show } from 'solid-js';
import type { Camera } from './camera-model';

export enum Tone {
  Ready = 'ready',
  Busy = 'busy',
  Problem = 'problem',
  Gone = 'gone',
}

function since(time: number) {
  const minutes = Math.floor((Date.now() - time) / 60000);
  return minutes < 1 ? 'just now' : `${minutes} min ago`;
}

/** One wording for a camera's state, shared by the strip and the card. */
export function cameraState(camera: Camera): { label: string; tone: Tone } {
  if (!camera.present)
    return {
      label: camera.goneAt === undefined ? 'Unplugged' : `Unplugged ${since(camera.goneAt)}`,
      tone: Tone.Gone,
    };
  if (camera.uncertain) return { label: 'Reconnect to continue', tone: Tone.Problem };
  const session = camera.session;
  if (session?.kind === 'local') return { label: session.tool, tone: Tone.Busy };
  if (session?.kind === 'remote')
    return {
      label: camera.remoteActivity?.title ?? `In use by ${session.reason || session.controller}`,
      tone: Tone.Busy,
    };
  return { label: 'Idle', tone: Tone.Ready };
}

const transportLabel: Record<Camera['transport'], string> = {
  usb: 'USB',
  remote: 'Remote',
  simulated: 'Simulated',
};

export function CameraStrip(props: {
  cameras: readonly Camera[];
  selected?: string;
  connecting: boolean;
  onSelect: (camera: string) => void;
  onConnect: () => void;
}) {
  return (
    <nav class="wb-cams" aria-label="Cameras">
      <For each={props.cameras}>
        {(camera) => {
          const state = () => cameraState(camera);
          return (
            <button
              class={{
                'wb-cam': true,
                selected: camera.id === props.selected,
                gone: !camera.present,
              }}
              aria-current={camera.id === props.selected ? 'true' : undefined}
              onClick={() => props.onSelect(camera.id)}
            >
              <span class="wb-cam-name">
                <i class={`wb-dot ${state().tone}`} aria-hidden="true" />
                {camera.name}
                <Show when={camera.sharing}>
                  {(sharing) => <span class="wb-tag">Shared · {sharing().peers.size}</span>}
                </Show>
              </span>
              <span class="wb-cam-meta">
                {transportLabel[camera.transport]} ·{' '}
                {camera.serviceMode ? 'Service mode · ' : ''}
                {state().label}
              </span>
            </button>
          );
        }}
      </For>
      <button class="wb-add" disabled={props.connecting} onClick={() => props.onConnect()}>
        {props.connecting ? 'Connecting…' : '+ Connect camera'}
      </button>
    </nav>
  );
}
