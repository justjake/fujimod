import { createSignal } from 'solid-js';

import { DesignFrame } from '../../../design/looks';
import { States } from '../../../design/screen';
import { SiteFrame } from '../../../ui/SiteFrame';
import { CustomSettingsApp } from './app';
import { SPLIT, createEditedLeicaFilm, createInitialState, type UnifiedState } from './model';
import { UnifiedEditor } from './unified-editor';

const ALL_OPEN = {
  raw: false,
  recipe: false,
  film: false,
  light: false,
  color: false,
  detail: false,
  effects: false,
};

const REVIEW_STATES: { id: string; title: string; state: UnifiedState }[] = [
  {
    id: 'collapsed',
    title: 'Collapsed casual view',
    state: createInitialState({
      focus: { kind: 'film-tiles' },
    }),
  },
  {
    id: 'film-expanded',
    title: 'Expanded film simulation well (Tone Curve ALL/R/G/B + LEICA X actions)',
    state: createInitialState({
      filmId: 'leica-x',
      film: createEditedLeicaFilm(),
      collapsed: ALL_OPEN,
      curveChannel: 'all',
      focus: { kind: 'film-curve' },
      inside: true,
      knot: 6,
      scrollTo: 'film',
    }),
  },
  {
    id: 'film-only',
    title: 'FILM ONLY isolation active',
    state: createInitialState({
      filmId: 'leica-x',
      film: createEditedLeicaFilm(),
      collapsed: ALL_OPEN,
      filmOnly: true,
      focus: { kind: 'film-luma' },
      scrollTo: 'luma',
    }),
  },
  {
    id: 'switch-modal',
    title: 'SAVE FILM CHANGES? modal on film switch',
    state: createInitialState({
      filmId: 'leica-x',
      film: createEditedLeicaFilm(),
      collapsed: ALL_OPEN,
      focus: { kind: 'film-tiles' },
      switchModal: { targetFilmId: 'velvia', focus: 0 },
      scrollTo: 'film',
    }),
  },
  {
    id: 'wb-shift',
    title: 'WB SHIFT 2D flyout',
    state: createInitialState({
      focus: { kind: 'setting', id: 'wb' },
      wbShift: { open: true, from: [2, -3] },
      scrollTo: 'color',
    }),
  },
  {
    id: 'raw-conversion',
    title: 'RAW CONVERSION mode',
    state: createInitialState({
      mode: 'raw',
      crop: 1,
      focus: { kind: 'raw', item: 'strip' },
      scrollTo: 'raw',
    }),
  },
  {
    id: 'compare',
    title: 'Compare A|B',
    state: createInitialState({
      filmId: 'leica-x',
      film: createEditedLeicaFilm(),
      compare: { b: 'opened', split: SPLIT },
      focus: { kind: 'film-tiles' },
    }),
  },
];

function ReviewScreen(props: { start: UnifiedState }) {
  const [state, setState] = createSignal(props.start);
  return <UnifiedEditor s={state()} set={setState} />;
}

/** /docs/design/custom-settings-editor/: the unified custom settings and film
 * simulation editor, live, followed by each review state. */
export default function CustomSettingsEditorPage() {
  return (
    <SiteFrame class="design-page">
      <DesignFrame title="Custom settings editor">
        <CustomSettingsApp />
        <States
          title="Review states"
          states={REVIEW_STATES}
          render={(state) => <ReviewScreen start={state} />}
        />
      </DesignFrame>
    </SiteFrame>
  );
}
