import { createSignal } from 'solid-js';

import { CameraKeys, CameraScreen, Fit, KeyHint, type CameraKey } from '../../../design/screen';
import {
  Editor as SettingsEditor,
  FILMS,
  INITIAL,
  apply,
  onKey,
  type EditorState,
} from './custom-settings';
import {
  Editor as FilmEditor,
  Picture,
  RAIL,
  SCROLLER,
  baseFilm,
  initial,
  press,
  useHistogram,
  type State as FilmState,
} from './film-editor';

/* The custom settings editor and the film editor are one app. Fn on the FILM
   SIMULATION row slides the film editor in from the left over the custom
   setting's rows; BACK from the film editor's rail slides it out again. */

const SPRING = '460ms cubic-bezier(0.22, 1, 0.36, 1)';
const FILM_ROW = 0;
const WB_ROW = 1;

/** The film editor's starting point for each stock film the custom setting can name. */
const FILM_BASE: Record<string, number> = {
  'PROVIA/STANDARD': 0,
  'Velvia/VIVID': 1,
  'ASTIA/SOFT': 2,
  'CLASSIC CHROME': 3,
  'PRO Neg. Hi': 4,
  'PRO Neg. Std': 4,
};

const SETTINGS_KEYS: Partial<Record<CameraKey, string>> = {
  up: 'ArrowUp',
  down: 'ArrowDown',
  left: 'ArrowLeft',
  right: 'ArrowRight',
  'dial-': 'DialLeft',
  'dial+': 'DialRight',
  ok: 'Enter',
  fn: 'Fn',
  back: 'Escape',
  q: 'q',
};

/** The custom settings editor with the film editor over it, live, with the
 * camera's keys. `connect` receives the app's key input for another page's
 * keys; `film` starts with the film editor open; `keys: false` leaves the
 * on-screen keys to that page. */
export function CustomSettingsApp(props: { connect?: (send: (key: CameraKey) => void) => void; film?: boolean; keys?: boolean }) {
  /* The film editor as Fn opens it over a custom setting's film. */
  const opened = (recipe: EditorState) => {
    const name = FILMS[recipe.recipe.film].name;
    const base = baseFilm(FILM_BASE[name] ?? 0);
    return initial({ film: base, opened: base, from: name, name: 'NEW FILM', area: 'rail', railOn: 'film' });
  };
  const [settings, setSettings] = createSignal<EditorState>(INITIAL);
  const [film, setFilm] = createSignal<FilmState>(props.film ? opened(INITIAL) : initial({}));
  const [open, setOpen] = createSignal(props.film === true);
  const histogram = useHistogram();

  const openFilm = () => {
    setFilm(opened(settings()));
    setOpen(true);
  };

  const send = (key: CameraKey) => {
    if (open()) {
      const next = press(film(), key);
      if (key === 'back' && next === film()) setOpen(false);
      else setFilm(next);
      return;
    }
    if (key === 'fn' && settings().focus === FILM_ROW) return openFilm();
    const mapped = SETTINGS_KEYS[key];
    if (mapped) setSettings(onKey(settings(), mapped));
  };
  props.connect?.(send);

  return (
    <>
      <Fit>
        <CameraScreen>
          <div
            class="absolute inset-0"
            style={{
              transition: `visibility 0s linear ${open() ? '460ms' : '0ms'}`,
              visibility: open() ? 'hidden' : 'visible',
            }}
          >
            <SettingsEditor
              state={settings()}
              send={(action) => setSettings(apply(settings(), action))}
              hint={
                settings().focus === FILM_ROW ? (
                  <KeyHint glyph="Fn" label="EDIT FILM" tap={openFilm} />
                ) : settings().focus === WB_ROW ? (
                  <KeyHint glyph="Fn" label="WB SHIFT" tap={() => setSettings(onKey(settings(), 'Fn'))} />
                ) : undefined
              }
            />
          </div>
          <div class="absolute inset-0" style={{ visibility: open() ? 'visible' : 'hidden', transition: `visibility 0s linear ${open() ? '0ms' : '460ms'}` }}>
            <div class="absolute inset-0" style={{ opacity: open() ? 1 : 0, transition: 'opacity 300ms' }}>
              <Picture film={film().film} custom={film().custom} crop={film().crop} />
            </div>
            <FilmEditor
              s={film()}
              set={setFilm}
              histogram={histogram()}
              column={{ transform: open() ? 'none' : `translateX(-${RAIL + SCROLLER}px)`, transition: `transform ${SPRING}` }}
              chrome={{ opacity: open() ? 1 : 0, transition: `opacity 200ms ${open() ? '200ms' : '0ms'}` }}
            />
          </div>
        </CameraScreen>
      </Fit>
      {props.keys !== false && <CameraKeys send={send} />}
    </>
  );
}
