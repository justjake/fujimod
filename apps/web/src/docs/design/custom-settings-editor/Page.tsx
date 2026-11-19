import { Show, createSignal } from 'solid-js';

import { DesignFrame } from '../../../design/looks';
import { CameraKeys, Fit, KeyHint, type CameraKey } from '../../../design/screen';
import { SiteFrame } from '../../../ui/SiteFrame';
import {
  Editor as SettingsEditor,
  FILMS,
  INITIAL,
  RecipeStates,
  apply,
  onKey,
  type EditorState,
} from './custom-settings';
import {
  Editor as FilmEditor,
  FilmStates,
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
  'dial-': 'ArrowLeft',
  'dial+': 'ArrowRight',
  ok: 'Enter',
  back: 'Escape',
  q: 'q',
};

function App() {
  const [settings, setSettings] = createSignal<EditorState>(INITIAL);
  const [film, setFilm] = createSignal<FilmState>(initial({}));
  const [open, setOpen] = createSignal(false);
  const histogram = useHistogram();

  const openFilm = () => {
    const name = FILMS[settings().recipe.film].name;
    const base = baseFilm(FILM_BASE[name] ?? 0);
    setFilm(initial({ film: base, opened: base, from: name, name: 'NEW FILM' }));
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

  return (
    <>
      <Fit>
        <div class="fm-screen relative h-[480px] w-[720px] overflow-hidden bg-black font-sans select-none">
          <div
            class="absolute inset-0"
            style={{
              transform: open() ? 'translateX(48px)' : 'none',
              opacity: open() ? 0 : 1,
              transition: `transform ${SPRING}, opacity ${open() ? '220ms' : '320ms 140ms'} ease-out, visibility 0s linear ${open() ? '460ms' : '0ms'}`,
              visibility: open() ? 'hidden' : 'visible',
            }}
          >
            <SettingsEditor state={settings()} send={(action) => setSettings(apply(settings(), action))} />
            <Show when={settings().focus === FILM_ROW}>
              <div class="absolute top-[10px] right-[10px]">
                <KeyHint glyph="Fn" label="EDIT FILM" tap={openFilm} />
              </div>
            </Show>
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
        </div>
      </Fit>
      <CameraKeys send={send} />
    </>
  );
}

/** /docs/design/custom-settings-editor/: the custom settings editor and the
 * film editor over it, live, then each screen state for review. */
export default function CustomSettingsEditorPage() {
  return (
    <SiteFrame class="design-page">
      <DesignFrame title="Custom settings editor">
        <App />
        <RecipeStates />
        <h2 class="mt-[32px] mb-[10px] text-[17px] font-semibold">Film editor</h2>
        <FilmStates />
      </DesignFrame>
    </SiteFrame>
  );
}
