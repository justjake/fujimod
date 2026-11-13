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

type Section = 'shooting' | 'recipes' | 'setup';
const SECTIONS: { id: Section; title: string }[] = [
  { id: 'shooting', title: 'SHOOTING' },
  { id: 'recipes', title: 'CUSTOM SETTINGS / RECIPES' },
  { id: 'setup', title: 'SET-UP' },
];
const GROUP_SECTIONS: Record<string, Section> = {
  photography: 'shooting',
  videography: 'shooting',
  recipes: 'recipes',
  system: 'setup',
};
/** Pages opened from a list entry rather than a menu row, by their title. */
const PAGE_SECTIONS: Record<string, Section> = {
  'CUSTOM SETTING': 'recipes',
  'COMPARE WITH CURRENT': 'recipes',
  CONTROL: 'setup',
};
/** Pages that open from a list entry, by the row whose entries open them. */
const OPENED_FROM: Record<string, string> = {
  'CUSTOM SETTING': 'LIBRARY',
  CONTROL: 'BUTTON/DIAL SETTING',
  'ASPECT RATIO': 'EDIT ASPECT RATIOS',
};
/** Rows whose own pages the outline leaves out. */
const COLLAPSED = new Set(['EDIT ASPECT RATIOS']);
const TOP_ORDER = ['SHOOTING MENU', 'PLAYBACK MENU', 'SET-UP'];
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

/** Effects and recipes, and the pages opened from them, join CUSTOM SETTINGS / RECIPES. */
function recipeRow(row: MenuRow) {
  const feature = row.feature ? features.get(row.feature) : undefined;
  return (
    feature?.group === 'recipes' ||
    rowPlace(row).some((title) => title.includes('CUSTOM SETTING')) ||
    PAGE_SECTIONS[row.path[0] ?? ''] === 'recipes'
  );
}

/** A row's section: recipes for an effect or a menu holding one, else its top menu's. */
function sectionOf(row: MenuRow): Section {
  if (recipeRow(row)) return 'recipes';
  const place = rowPlace(row).join('\u0000') + '\u0000';
  const holds = shownRows.some(
    (other) => rowPlace(other).join('\u0000').startsWith(place) && recipeRow(other),
  );
  const nested = shownRows.some(
    (other) =>
      rowPlace(other).join('\u0000').startsWith(place) && !!other.feature && !recipeRow(other),
  );
  if (holds && !nested) return 'recipes';
  const top = row.path[0] ?? '';
  if (TOP_MENUS.has(top)) return top === 'SET-UP' ? 'setup' : 'shooting';
  if (PAGE_SECTIONS[top]) return PAGE_SECTIONS[top]!;
  const feature = row.feature ? features.get(row.feature) : undefined;
  return feature ? GROUP_SECTIONS[feature.group]! : 'shooting';
}

/** One node of a section's menu tree; `row` is absent for menus FujiMod only passes through. */
type Node = { title: string; row?: MenuRow; children: Map<string, Node> };

/** The deepest node with this title: the page a partial path's first title opens. */
function opener(root: Node, title: string): Node | undefined {
  const from = OPENED_FROM[title];
  if (from) {
    const list = opener(root, from);
    if (!list) return undefined;
    if (!list.children.has(title)) list.children.set(title, { title, children: new Map() });
    return list.children.get(title);
  }
  let best: { node: Node; depth: number } | undefined;
  const visit = (node: Node, depth: number) => {
    for (const child of node.children.values()) {
      if (child.title === title && (!best || depth > best.depth)) best = { node: child, depth };
      visit(child, depth + 1);
    }
  };
  visit(root, 0);
  return best?.node;
}

function insert(start: Node, titles: readonly string[], row: MenuRow) {
  let node = start;
  for (const title of titles) {
    let next = node.children.get(title);
    if (!next) node.children.set(title, (next = { title, children: new Map() }));
    node = next;
  }
  node.row = row;
}

/** The section's menu tree; a page reached from a list entry hangs under the row it opens from. */
function tree(rows: readonly MenuRow[]) {
  const root: Node = { title: '', children: new Map() };
  const rooted = (row: MenuRow) => TOP_MENUS.has(row.path[0] ?? '');
  for (const row of rows.filter(rooted)) insert(root, rowPlace(row), row);
  // A page can open from another such page, so place those whose opener is placed first.
  let waiting = rows.filter((row) => !rooted(row));
  while (waiting.length) {
    const placed = waiting.filter((row) => {
      const [head, ...rest] = rowPlace(row);
      const page = opener(root, head!);
      if (page) insert(page, rest, row);
      return page;
    });
    if (!placed.length) break;
    waiting = waiting.filter((row) => !placed.includes(row));
  }
  for (const row of waiting) insert(root, rowPlace(row), row);
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
      heading = { ...child, children: new Map() };
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
  const feature = (): Feature | undefined => (cardKey() ? tiles.get(cardKey()!) : undefined);
  const added = () => !!props.node.row && !props.node.row.stock;
  const credits = () => feature()?.credits ?? card()?.credits ?? NO_CREDITS;
  /** One description: the card's, else its stock change, else the tile's. */
  const text = () => card()?.summary ?? card()?.changes ?? feature()?.summary;
  const choices = () => (card() ? choicesOf(cardKey()!, card()!) : []);
  const switched = () => isSwitch(choices());
  return (
    <li class={{ heading: props.node.row?.kind === 'heading' }}>
      <div class="outline-row">
        <Show when={props.node.row?.icon ?? feature()?.icon}>
          {(name) => <FeatureIcon name={name()} />}
        </Show>
        <span class="outline-title">{props.node.title}</span>
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
      <Show when={card() && choices().length && !switched()}>
        <Options choices={choices()} card={card()!} />
      </Show>
      <CreditList credits={credits()} />
      <Show when={props.node.children.size && !COLLAPSED.has(props.node.title)}>
        <ul>
          <For each={ordered(props.node)}>
            {(child) => <OutlineNode node={child} inside={props.inside || added()} />}
          </For>
        </ul>
      </Show>
    </li>
  );
}

function MenuFree(props: { features: Feature[] }) {
  return (
    <Show when={props.features.length}>
      <li class="heading">
        <div class="outline-row">
          <span class="outline-title">Outside the menus</span>
        </div>
        <ul>
          <For each={props.features}>
            {(feature) => (
              <li>
                <div class="outline-row">
                  <FeatureIcon name={feature.icon} />
                  <span class="outline-title">{[...feature.path, feature.title].join(' > ')}</span>
                  <span class="outline-new">NEW</span>
                </div>
                <p class="outline-text">
                  <Prose text={feature.summary} />
                </p>
                <CreditList credits={feature.credits} />
              </li>
            )}
          </For>
        </ul>
      </li>
    </Show>
  );
}

/** Every menu row FujiMod adds, with its documentation, in three sections. */
export function FeatureOutline() {
  return (
    <section class="outline" aria-labelledby="outline">
      <h2 class="section-heading" id="outline">
        Everything in FujiMod
      </h2>
      <For each={SECTIONS}>
        {(section) => {
          const rows = shownRows.filter((row) => sectionOf(row) === section.id);
          const free = catalog.features.filter(
            (feature) => !placed.has(feature.id) && GROUP_SECTIONS[feature.group] === section.id,
          );
          return (
            <details class="outline-section" open>
              <summary>{section.title}</summary>
              <ul class="outline-tree">
                <For each={ordered(tree(rows))}>
                  {(node) => <OutlineNode node={node} inside={false} />}
                </For>
                <MenuFree features={free} />
              </ul>
            </details>
          );
        }}
      </For>
    </section>
  );
}
