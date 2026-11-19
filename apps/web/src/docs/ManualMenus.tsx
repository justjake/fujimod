import { For, Show } from 'solid-js';
import { manualMenuRows } from './manual-data';
import { ManualFeatureLink } from '../ui/ManualFeatureLink';
import { manualRoot } from './manual-routes';

type MenuRow = (typeof manualMenuRows)[number];
type MenuNode = {
  title: string;
  row?: MenuRow;
  children: Map<string, MenuNode>;
};

// The generator emits camera order. A parent without its own row is a group heading.
const menus = new Map<string, MenuNode>();
for (const row of manualMenuRows) {
  let siblings = menus;
  for (const title of row.path) {
    let node = siblings.get(title);
    if (!node) siblings.set(title, (node = { title, children: new Map() }));
    siblings = node.children;
  }
  const node = siblings.get(row.title);
  if (node) node.row = row;
  else siblings.set(row.title, { title: row.title, row, children: new Map() });
}

function MenuBranches(props: { nodes: Map<string, MenuNode> }) {
  return (
    <ul>
      <For each={[...props.nodes.values()]}>
        {(node) => (
          <li>
            <Show when={node.row} fallback={<span class="manual-menu-group">{node.title}</span>}>
              {(row) => (
                <ManualFeatureLink page={row().page} row={row().anchor}>
                  {node.title}
                </ManualFeatureLink>
              )}
            </Show>
            <Show when={node.children.size}>
              <MenuBranches nodes={node.children} />
            </Show>
          </li>
        )}
      </For>
    </ul>
  );
}

export function ManualMenus() {
  return (
    <article class="manual-index">
      <p class="manual-crumb">
        <a href={manualRoot}>Manual</a>
      </p>
      <h1>Menu index</h1>
      <p class="manual-lede">
        Every menu item Fujimod adds or changes, and the manual page that covers it.
      </p>
      <nav class="manual-menu-tree" aria-label="Camera menus">
        <MenuBranches nodes={menus} />
      </nav>
    </article>
  );
}
