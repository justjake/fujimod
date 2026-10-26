import { Show } from 'solid-js';
import type { LCDViewer } from './lcd';
import { InputBadge, badgeCSS } from '../feedback/input';
export function ViewerOverlays(props: { viewer?: LCDViewer }) {
  return (
    <div
      class="lcd-input-feedback"
      role="status"
      aria-live="polite"
      hidden={!props.viewer?.ui.badge}
      data-input={props.viewer?.ui.badge?.id}
      aria-label={props.viewer?.ui.badge?.label}
      style={props.viewer?.ui.badge ? badgeCSS : {}}
    >
      <Show when={props.viewer?.ui.badge}>
        {(badge) => <InputBadge id={badge().id} label={badge().label} />}
      </Show>
    </div>
  );
}
