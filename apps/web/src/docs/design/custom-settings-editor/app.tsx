import { createSignal } from 'solid-js';

import { CameraKeys, CameraScreen, Fit, type CameraKey } from '../../../design/screen';
import { createInitialState, press, type UnifiedState } from './model';
import { UnifiedEditor } from './unified-editor';

/** The unified Custom Settings + Film Simulation Editor, live, with the
 * camera's keys. `connect` receives the app's key input for another page's
 * keys; `film` starts with the Film Simulation tools expanded; `keys: false`
 * leaves the on-screen keys to that page. */
export function CustomSettingsApp(props: {
  connect?: (send: (key: CameraKey) => void) => void;
  film?: boolean;
  keys?: boolean;
}) {
  const [state, setState] = createSignal<UnifiedState>(
    props.film
      ? createInitialState({
          collapsed: {
            raw: false,
            recipe: false,
            film: false,
            light: false,
            color: false,
            detail: false,
            effects: false,
          },
          focus: { kind: 'film-curve' },
          scrollTo: 'film',
        })
      : createInitialState(),
  );

  const send = (key: CameraKey) => {
    setState((prev) => press(prev, key));
  };
  props.connect?.(send);

  return (
    <>
      <Fit>
        <CameraScreen>
          <UnifiedEditor s={state()} set={setState} />
        </CameraScreen>
      </Fit>
      {props.keys !== false && <CameraKeys send={send} />}
    </>
  );
}
