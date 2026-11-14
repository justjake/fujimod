import { For, Show } from 'solid-js';
import {
  catalog,
  type Card,
  type Choice,
  type Credits,
  type Feature,
  type MenuRow,
} from './catalog';
import { ChipIcon, CreditList, Premiere, Prose } from './catalog-parts';
import { FeatureIcon } from './FeatureIcon';
import './outline.css';

/** Top menus, and the DRIVE button's popup, in the order the outline lists them. */
const TOP_ORDER = ['DRIVE', 'SHOOTING MENU', 'SET-UP', 'SYSTEM CHANGES', 'PLAYBACK MENU'];
/** Top menus per desktop column: shooting, set-up, then system and playback. */
const COLUMNS = [
  ['DRIVE', 'SHOOTING MENU'],
  ['SET-UP'],
  ['SYSTEM CHANGES', 'PLAYBACK MENU'],
];
const TOP_MENUS = new Set(TOP_ORDER);

const features = new Map(catalog.features.map((feature) => [feature.id, feature]));
/** The feature whose tile is this card's menu row. */
const tiles = new Map(
  catalog.features.filter((feature) => feature.card).map((feature) => [feature.card!, feature]),
);
/** Each card's options with their stock flag and icon, from the features that list its control. */
const options = new Map<string, Choice[]>();
for (const feature of catalog.features)
  for (const control of feature.controls)
    if (control.card && !options.has(control.card)) options.set(control.card, control.choices);
const rowPlace = (row: MenuRow) => [...row.path, row.title];
/** Stock rows FujiMod leaves as they are. */
const shownRows = catalog.menus.filter((row) => !row.stock || row.card);
const placed = new Set(shownRows.map((row) => row.feature));
const placeKey = (place: readonly string[]) => place.join('\u0000');
/** Features without a row of their own that change a row: listed on it (RAW on DIGITAL TELE-CONV.). */
const attached = new Map<string, Feature[]>();
/** Features without a row whose path still leads to a shown row are not menu-free. */
const reached = new Set<string>();
for (const feature of catalog.features) {
  if (placed.has(feature.id) || !feature.path.length) continue;
  const at = placeKey(feature.path);
  if (!shownRows.some((row) => placeKey(rowPlace(row)).startsWith(at))) continue;
  reached.add(feature.id);
  if (feature.attached) attached.set(at, [...(attached.get(at) ?? []), feature]);
}

/** One node of the menu tree; `row` is absent for menus FujiMod only passes through. */
type Node = { title: string; row?: MenuRow; children: Map<string, Node> };

function insert(start: Node, titles: readonly string[], row: MenuRow) {
  let node = start;
  for (const title of titles) {
    let next = node.children.get(title);
    if (!next) node.children.set(title, (next = { title, children: new Map() }));
    node = next;
  }
  node.row = row;
}

/** The menu tree. The docs place every row under a top menu; any other row stays at the root. */
function tree(rows: readonly MenuRow[]) {
  const root: Node = { title: '', children: new Map() };
  for (const row of rows) insert(root, rowPlace(row), row);
  return root;
}

/** Children in menu order, with the rows after each heading gathered under it. */
function ordered(node: Node): Node[] {
  const rank = (child: Node) =>
    child.row ? child.row.order : TOP_MENUS.has(child.title) ? TOP_ORDER.indexOf(child.title) : 0;
  const children = [...node.children.values()].sort((a, b) => rank(a) - rank(b));
  const out: Node[] = [];
  let heading: Node | undefined;
  for (const child of children) {
    if (child.row?.kind === 'heading') {
      heading = { ...child, children: new Map(child.children) };
      out.push(heading);
    } else if (heading) heading.children.set(child.title + heading.children.size, child);
    else out.push(child);
  }
  return out;
}

const NO_CREDITS: Credits = { premiere: false, inspired: [], thanks: [] };

function Kind(props: { row?: MenuRow; inside: boolean }) {
  return (
    <>
      <Show when={props.row?.kind === 'command'}>
        <span class="outline-badge">cmd</span>
      </Show>
      <Show when={props.row && !props.row.stock && !props.inside}>
        <span class="outline-new">NEW</span>
      </Show>
    </>
  );
}

/** The card's options, or the control's from the features that list it. */
function choicesOf(cardKey: string, card: Card): Choice[] {
  return (
    options.get(cardKey) ??
    Object.keys(card.choices).map((label) => ({ label, stock: false, icon: null }))
  );
}

/** A plain switch reads as ON/OFF beside its title. */
const isSwitch = (choices: readonly Choice[]) =>
  choices.length === 2 &&
  choices.every((choice) => choice.label === 'ON' || choice.label === 'OFF');

/** Option chips on one line, as on the catalog tiles; each option's text is its tooltip. */
function Options(props: { choices: Choice[]; card: Card }) {
  return (
    <div class="catalog-chips outline-chips">
      <For each={props.choices}>
        {(choice) => (
          <span
            class={choice.stock ? 'catalog-chip stock' : 'catalog-chip'}
            title={props.card.choices[choice.label]}
          >
            <Show when={choice.icon}>{(name) => <ChipIcon name={name()} />}</Show>
            {choice.label}
          </span>
        )}
      </For>
    </div>
  );
}

/** A menu row with everything the catalog knows about it. `inside` marks rows within a row FujiMod adds. */
function OutlineNode(props: { node: Node; inside: boolean }) {
  const cardKey = () => props.node.row?.card ?? undefined;
  const card = () => (cardKey() ? catalog.cards[cardKey()!] : undefined);
  /** The feature whose tile is this row, or that the row stands for (DRIVE > SOFTWARE ND). */
  const feature = (): Feature | undefined => {
    if (cardKey()) return tiles.get(cardKey()!);
    const own = props.node.row?.feature ? features.get(props.node.row.feature) : undefined;
    const stands = props.node.row?.card?.startsWith('row:') || own?.title === props.node.title;
    return own && (!own.card || props.node.row?.card?.startsWith('row:')) && stands ? own : undefined;
  };
  const added = () => !!props.node.row && !props.node.row.stock;
  const credits = () => feature()?.credits ?? card()?.credits ?? NO_CREDITS;
  /** One description: what FujiMod changes in a stock row, else the card's, else the tile's. */
  const text = () => card()?.changes ?? card()?.summary ?? feature()?.summary;
  const also = () => (props.node.row ? (attached.get(placeKey(rowPlace(props.node.row))) ?? []) : []);
  const choices = () => (card() ? choicesOf(cardKey()!, card()!) : []);
  const switched = () => isSwitch(choices());
  return (
    <li class={{ heading: props.node.row?.kind === 'heading' }}>
      <div class="outline-row">
        <Show when={props.node.row?.icon ?? feature()?.icon}>
          {(name) => <FeatureIcon name={name()} />}
        </Show>
        <Show
          when={/^\{(.+)\}$/.exec(props.node.title)?.[1]}
          fallback={<span class="outline-title">{props.node.title}</span>}
        >
          {(entry) => <span class="outline-title outline-entry">chosen {entry()}</span>}
        </Show>
        <Show when={switched()}>
          <span class="outline-switch">ON/OFF</span>
        </Show>
        <Show when={card()?.drive_mode}>
          <FeatureIcon name="drive-single" />
        </Show>
        <Kind row={props.node.row} inside={props.inside} />
        <Show when={credits().premiere}>
          <Premiere />
        </Show>
      </div>
      <Show when={text()}>
        {(found) => (
          <p class="outline-text">
            <Prose text={found()} />
          </p>
        )}
      </Show>
      <For each={also()}>
        {(extra) => (
          <p class="outline-text">
            <Prose text={extra.summary} />
          </p>
        )}
      </For>
      <Show when={card() && choices().length && !switched()}>
        <Options choices={choices()} card={card()!} />
      </Show>
      <CreditList credits={credits()} />
      <Show when={props.node.children.size}>
        <ul>
          <For each={ordered(props.node)}>
            {(child) => <OutlineNode node={child} inside={props.inside || added()} />}
          </For>
        </ul>
      </Show>
    </li>
  );
}

/** Features no menu row reaches, such as button actions. */
function MenuFree(props: { features: Feature[] }) {
  return (
    <For each={props.features}>
      {(feature) => (
        <li>
          <div class="outline-row">
            <FeatureIcon name={feature.icon} />
            <span class="outline-title">{feature.title}</span>
            <span class="outline-new">NEW</span>
          </div>
          <p class="outline-text">
            <Prose text={feature.summary} />
          </p>
          <CreditList credits={feature.credits} />
        </li>
      )}
    </For>
  );
}

/** Every menu row FujiMod adds, with its documentation, under the camera's own top menus. */
export function FeatureOutline() {
  const free = catalog.features.filter(
    (feature) => !placed.has(feature.id) && !reached.has(feature.id),
  );
  const tops = ordered(tree(shownRows));
  return (
    <section class="outline" aria-labelledby="outline">
      <h2 class="section-heading" id="outline">
        Everything in FujiMod
      </h2>
      <div class="outline-columns">
        <For each={COLUMNS}>
          {(column, index) => (
            <div class="outline-column">
              <For each={tops.filter((top) => column.includes(top.title))}>
                {(top) => (
                  <details class="outline-section" open>
                    <summary>{top.title}</summary>
                    <ul class="outline-tree">
                      <For each={ordered(top)}>{(node) => <OutlineNode node={node} inside={false} />}</For>
                    </ul>
                  </details>
                )}
              </For>
              <Show when={index() === COLUMNS.length - 1 && free.length}>
                <details class="outline-section" open>
                  <summary>OUTSIDE THE MENUS</summary>
                  <ul class="outline-tree">
                    <MenuFree features={free} />
                  </ul>
                </details>
              </Show>
            </div>
          )}
        </For>
      </div>
    </section>
  );
}
