import { Show, createSignal, onSettled } from 'solid-js';
import { Icon } from './ui/icons';
import { SpeedGauge } from './ui/speed';
import type { EmulatorFeedbackActions } from './Feedback';
import './viewport-controls.css';

/** The short bar under the display: camera, firmware picker, speed, webcam and feedback. */
export function ViewportControls(props: {
  active: boolean;
  title: string;
  speed?: number;
  webcamSupported?: boolean;
  webcamActive?: boolean;
  webcamBusy?: boolean;
  webcamDisabled?: boolean;
  webcamError?: string;
  toggleWebcam?: () => void;
  modSupported: boolean;
  modEnabled: boolean;
  modDisabled: boolean;
  feedback: EmulatorFeedbackActions;
  selectMod: (enabled: boolean) => void;
}) {
  const [picking, setPicking] = createSignal(false);
  const [fullscreen, setFullscreen] = createSignal(false);
  let bar: HTMLDivElement | undefined;
  let picker: HTMLDivElement | undefined;
  let trigger: HTMLButtonElement | undefined;
  function close() {
    setPicking(false);
    trigger?.focus();
  }
  onSettled(() => {
    const outside = (event: PointerEvent) => {
      if (picker && event.target instanceof Node && !picker.contains(event.target))
        setPicking(false);
    };
    const changed = () =>
      setFullscreen(
        !!document.fullscreenElement || !!stage()?.classList.contains('stage-fullscreen'),
      );
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && stage()?.classList.contains('stage-fullscreen'))
        toggleFullscreen();
    };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('fullscreenchange', changed);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('pointerdown', outside);
      document.removeEventListener('fullscreenchange', changed);
      document.removeEventListener('keydown', escape);
    };
  });
  const stage = () => bar?.closest<HTMLElement>('.stage') ?? undefined;
  /** Full screen holds the display and its controls. The Fullscreen API is used where the browser
   * offers it for elements; iPhone Safari gets a page-filling stage. */
  function toggleFullscreen() {
    const element = stage();
    if (!element) return;
    if (document.fullscreenElement) void document.exitFullscreen();
    else if (element.classList.contains('stage-fullscreen')) {
      element.classList.remove('stage-fullscreen');
      setFullscreen(false);
    } else if (document.fullscreenEnabled && element.requestFullscreen)
      void element.requestFullscreen();
    else {
      element.classList.add('stage-fullscreen');
      setFullscreen(true);
    }
  }
  const choose = (enabled: boolean) => {
    close();
    if (enabled !== props.modEnabled) props.selectMod(enabled);
  };
  return (
    <div
      ref={bar}
      id="viewer-tools"
      class="viewport-bar"
      data-ready={String(props.feedback.ready)}
      role="toolbar"
      aria-label="Camera"
      // An idle viewer shows the bar only to recover a saved report.
      hidden={!props.active && !props.feedback.saved}
    >
      <Show when={props.active}>
        <span id="camera-title">{props.title}</span>
        <Show when={props.modSupported}>
          <div
            ref={picker}
            id="firmware-mode"
            class="firmware-picker"
            onKeyDown={(event) => {
              if (event.key === 'Escape' && picking()) {
                event.preventDefault();
                event.stopPropagation();
                close();
              }
            }}
          >
            <button
              ref={trigger}
              type="button"
              aria-haspopup="menu"
              aria-expanded={picking() ? 'true' : 'false'}
              aria-label="Firmware"
              disabled={props.modDisabled}
              onClick={() => setPicking(!picking())}
            >
              {props.modEnabled ? 'FujiMod' : 'Unmodded'}
              <Icon name="chevron" size={12} />
            </button>
            <Show when={picking()}>
              <div class="firmware-menu" role="menu" aria-label="Firmware">
                {[
                  { value: true, label: 'FujiMod' },
                  { value: false, label: 'Unmodded' },
                ].map((option) => (
                  <button
                    type="button"
                    role="menuitemradio"
                    aria-checked={props.modEnabled === option.value ? 'true' : 'false'}
                    data-mod-enabled={String(option.value)}
                    onClick={() => choose(option.value)}
                  >
                    <span class="firmware-check">
                      {props.modEnabled === option.value ? '✓' : ''}
                    </span>
                    {option.label}
                  </button>
                ))}
              </div>
            </Show>
          </div>
        </Show>
        <Show when={props.speed !== undefined && Number.isFinite(props.speed)}>
          <SpeedGauge speed={props.speed!} />
        </Show>
      </Show>
      <span class="viewport-bar-spacer" />
      <Show when={props.active && props.webcamSupported}>
        <Show when={props.webcamError}>
          <span class="webcam-error" role="status">
            {props.webcamError}
          </span>
        </Show>
        <button
          id="webcam-toggle"
          type="button"
          aria-label={
            props.webcamActive || props.webcamBusy ? 'Disable webcam input' : 'Enable webcam input'
          }
          title={
            props.webcamActive || props.webcamBusy ? 'Disable webcam input' : 'Enable webcam input'
          }
          aria-pressed={props.webcamActive ? 'true' : 'false'}
          aria-busy={props.webcamBusy ? 'true' : 'false'}
          disabled={props.webcamDisabled}
          onClick={() => props.toggleWebcam?.()}
        >
          <Icon name="camera" size={16} />
        </button>
      </Show>
      <Show when={props.active}>
        <button
          type="button"
          id="feedback-open"
          aria-label="Report a bug or idea"
          title="Report a bug or idea"
          disabled={!props.feedback.ready}
          onClick={() => props.feedback.open()}
        >
          <Icon name="bug" size={16} />
        </button>
      </Show>
      <Show when={props.feedback.saved}>
        <button
          type="button"
          id="feedback-recover"
          aria-label="Download saved report"
          title="Download saved report"
          onClick={() => props.feedback.recover()}
        >
          <Icon name="download" size={16} />
        </button>
      </Show>
      <Show when={props.active}>
        <button
          type="button"
          id="fullscreen-toggle"
          aria-label={fullscreen() ? 'Exit full screen' : 'Full screen'}
          title={fullscreen() ? 'Exit full screen' : 'Full screen'}
          aria-pressed={fullscreen() ? 'true' : 'false'}
          onClick={toggleFullscreen}
        >
          <Icon name={fullscreen() ? 'minimize' : 'maximize'} size={16} />
        </button>
      </Show>
    </div>
  );
}
