import { For, Show } from 'solid-js';
import { catalog, type Card, type Feature, type MenuRow } from './catalog';
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

const features = new Map(catalog.features.map((feature) => [feature.id, feature]));
const rowPlace = (row: MenuRow) => [...row.path, row.title];

function ruledSection(row: MenuRow): Section | undefined {
  const feature = row.feature ? features.get(row.feature) : undefined;
  if (feature) return GROUP_SECTIONS[feature.group];
  const place = rowPlace(row);
  if (place.some((title) => title.includes('CUSTOM SETTING'))) return 'recipes';
  if (PAGE_SECTIONS[place[0]!]) return PAGE_SECTIONS[place[0]!];
  return undefined;
}

/** A row without a rule takes the section of its first ruled descendant, else its top menu's. */
function sectionOf(row: MenuRow): Section {
  const ruled = ruledSection(row);
  if (ruled) return ruled;
  const place = rowPlace(row).join('\u0000');
  const child = catalog.menus.find(
    (other) =>
      other !== row &&
      rowPlace(other)
        .join('\u0000')
        .startsWith(place + '\u0000') &&
      ruledSection(other),
  );
  if (child) return ruledSection(child)!;
  return row.path[0] === 'SET-UP' ? 'setup' : 'shooting';
}

/** One node of a section's menu tree. */
type Node = { title: string; card?: Card; children: Map<string, Node> };

const TOP_MENUS = new Set(['SHOOTING MENU', 'SET-UP', 'PLAYBACK MENU']);

/** Pages that open from a list entry, by the row whose entries open them. */
const OPENED_FROM: Record<string, string> = { 'CUSTOM SETTING': 'LIBRARY' };

/** The deepest node with this title: the page a partial path's first title opens. */
function opener(root: Node, title: string): Node | undefined {
  const from = OPENED_FROM[title];
  if (from) {
    const list = opener(root, from);
    return (
      list &&
      (list.children.get(title) ??
        list.children.set(title, { title, children: new Map() }).get(title))
    );
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

function insert(start: Node, titles: readonly string[], card: string | null) {
  let node = start;
  for (const title of titles) {
    let next = node.children.get(title);
    if (!next) node.children.set(title, (next = { title, children: new Map() }));
    node = next;
  }
  if (card) node.card = catalog.cards[card];
}

/** The section's menu tree; a page reached from a list entry hangs under the row it opens from. */
function tree(rows: readonly MenuRow[]) {
  const root: Node = { title: '', children: new Map() };
  const rooted = (row: MenuRow) => TOP_MENUS.has(row.path[0] ?? '');
  for (const row of rows.filter(rooted)) insert(root, rowPlace(row), row.card);
  // A page can open from another such page, so place those whose opener is placed first.
  let waiting = rows.filter((row) => !rooted(row));
  while (waiting.length) {
    const placed = waiting.filter((row) => {
      const [head, ...rest] = rowPlace(row);
      const page = opener(root, head!);
      if (page) insert(page, rest, row.card);
      return page;
    });
    if (!placed.length) break;
    waiting = waiting.filter((row) => !placed.includes(row));
  }
  for (const row of waiting) insert(root, rowPlace(row), row.card);
  return root;
}

/** Labels of the options FujiMod adds to a control, from the features that list it. */
const added = new Map<string, Set<string>>();
for (const feature of catalog.features)
  for (const control of feature.controls)
    if (control.card)
      for (const choice of control.choices)
        if (!choice.stock)
          (added.get(control.card) ?? added.set(control.card, new Set()).get(control.card)!).add(
            choice.label,
          );

function OutlineNode(props: { node: Node }) {
  const cardKey = () =>
    Object.entries(catalog.cards).find(([, card]) => card === props.node.card)?.[0];
  return (
    <li>
      <span class="outline-title">{props.node.title}</span>
      <Show when={props.node.card?.summary}>
        <span class="outline-summary">{props.node.card!.summary}</span>
      </Show>
      <Show when={props.node.card && Object.keys(props.node.card.choices).length}>
        <ul class="outline-choices">
          <For each={Object.entries(props.node.card!.choices)}>
            {([label, text]) => (
              <li>
                <span class="outline-choice">{label}</span>
                <Show when={cardKey() && added.get(cardKey()!)?.has(label)}>
                  <span class="outline-new">NEW</span>
                </Show>{' '}
                <span class="outline-summary">{text}</span>
              </li>
            )}
          </For>
        </ul>
      </Show>
      <Show when={props.node.children.size}>
        <ul>
          <For each={[...props.node.children.values()]}>
            {(child) => <OutlineNode node={child} />}
          </For>
        </ul>
      </Show>
    </li>
  );
}

function MenuFree(props: { features: Feature[] }) {
  return (
    <Show when={props.features.length}>
      <li>
        <span class="outline-title">Without a menu</span>
        <ul>
          <For each={props.features}>
            {(feature) => (
              <li>
                <span class="outline-title">{feature.title}</span>
                <span class="outline-summary">{feature.summary}</span>
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
          const rows = catalog.menus.filter((row) => sectionOf(row) === section.id);
          const free = catalog.features.filter(
            (feature) => !feature.path.length && GROUP_SECTIONS[feature.group] === section.id,
          );
          return (
            <details class="outline-section" open>
              <summary>{section.title}</summary>
              <ul class="outline-tree">
                <For each={[...tree(rows).children.values()]}>
                  {(node) => <OutlineNode node={node} />}
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
