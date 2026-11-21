import { Show } from 'solid-js';
import { Icon } from '../ui/icons';

export function PowerOff(props: { visible: boolean; turnOn(): void }) {
  return (
    <Show when={props.visible}>
      <div class="camera-power-off">
        <p role="status">Camera is powered off</p>
        <button type="button" class="pill primary" onClick={() => props.turnOn()}>
          <Icon name="power" size={18} />
          Power off and on
        </button>
      </div>
    </Show>
  );
}
