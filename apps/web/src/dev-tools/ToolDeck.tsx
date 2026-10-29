import { BatteryCycleIcon } from '../ui/battery-cycle';
import { For, Show } from 'solid-js';
import type { JSX } from '@solidjs/web';
import './tool-deck.css';

function Tile(props: {
  title: string;
  locked?: string;
  /** Unavailable for a reason shown elsewhere on the page. */
  disabled?: boolean;
  busy?: boolean;
  hero?: boolean;
  onOpen?: () => void;
  href?: string;
  children?: JSX.Element;
}) {
  const content = () => (
    <>
      <span class="td-title">{props.title}</span>
      <Show when={props.children}>
        <span class="td-description">{props.children}</span>
      </Show>
      <Show when={props.locked}>
        <span class="td-lock">
          <span aria-hidden="true">■</span> {props.locked}
        </span>
      </Show>
      <Show when={props.busy}>
        <span class="td-status">Connecting…</span>
      </Show>
    </>
  );
  return (
    <Show
      when={props.href}
      fallback={
        <button
          type="button"
          class={{ 'td-action': true, 'td-tile': props.hero, 'td-card': !props.hero }}
          aria-label={props.title}
          aria-busy={props.busy ? 'true' : 'false'}
          disabled={Boolean(props.locked) || props.disabled || props.busy}
          onClick={() => props.onOpen?.()}
        >
          {content()}
        </button>
      }
    >
      <a
        class={{ 'td-action': true, 'td-tile': props.hero, 'td-card': !props.hero }}
        aria-label={props.title}
        href={props.href}
      >
        {content()}
      </a>
    </Show>
  );
}
/** Dev Tools start screen, shown until a camera is connected. */
/** A camera saved in the emulator, offered as a simulated camera. */
export interface SavedCamera {
  id: string;
  name: string;
}
export function ToolDeck(props: {
  busy?: boolean;
  usbSupported?: boolean;
  /** Undefined while the saved cameras are loading. */
  savedCameras?: readonly SavedCamera[];
  simulationAvailable: boolean;
  error?: string;
  onServiceMode: () => void;
  onConnect: () => void;
  onSimulated: (savedCamera: string) => void;
}) {
  return (
    <section class="td" aria-labelledby="td-title">
      <h1 id="td-title" class="visually-hidden">
        Dev Tools
      </h1>
      <Show when={!props.usbSupported}>
        <p class="td-unsupported" role="status">
          This browser can’t connect to USB. Use Chrome or Edge.
        </p>
      </Show>
      <Show when={props.error}>
        <p class="td-unsupported" role="alert">
          {props.error}
        </p>
      </Show>
      <div class="td-hero">
        <article class="td-tile td-enter">
          <h2>Enter service mode</h2>
          <p>
            Service mode exposes the camera’s ff80 USB connection. Switching the camera off in
            service mode restores factory settings. <BatteryCycleIcon />
            Remove the battery to leave service mode.
          </p>
          <button class="td-cta" onClick={() => props.onServiceMode()}>
            Show instructions
          </button>
        </article>
        <div class="td-connect-group">
          <div class="td-connects">
            <Tile
              hero
              title="Connect camera"
              disabled={!props.usbSupported}
              busy={props.busy}
              onOpen={() => props.onConnect()}
            >
              A camera in service mode or debug mode, over USB.
            </Tile>
            <article class="td-tile td-simulated" aria-labelledby="td-simulated">
              <h2 id="td-simulated" class="td-title">
                Simulated camera
              </h2>
              <Show when={!props.simulationAvailable}>
                <span class="td-lock">
                  <span aria-hidden="true">■</span> Simulated USB isn’t in this build yet.
                </span>
              </Show>
              <Show
                when={props.savedCameras?.length}
                fallback={
                  <Show when={props.savedCameras}>
                    <p class="td-empty">
                      No saved cameras. <a href="../">Add one in the emulator</a>
                    </p>
                  </Show>
                }
              >
                <div class="td-saved" role="group" aria-label="Saved cameras">
                  <For each={props.savedCameras}>
                    {(camera) => (
                      <button
                        class="td-saved-camera"
                        disabled={!props.simulationAvailable || props.busy}
                        onClick={() => props.onSimulated(camera.id)}
                      >
                        {camera.name}
                      </button>
                    )}
                  </For>
                </div>
              </Show>
            </article>
          </div>
        </div>
      </div>
      <a class="td-footer-link" href="../bug/">
        Bug reports
      </a>
    </section>
  );
}
