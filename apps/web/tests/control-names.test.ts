import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const read = (path: string) => JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8'));
test('X70 panel presentation uses semantic body-control IDs', () => {
  const panel = read('../devices/x70/panel.json');
  const { controls, icons } = read('../../../catalog/input-presentation.json');
  const ids = new Set<string>();
  const visit = (nodes: import('@fujimod/emulator-ui/ui/layout').ControlNode[]) => {
    for (const node of nodes) {
      if (node.id) ids.add(node.id);
      for (const id of Object.values(node.bindings ?? {})) ids.add(id);
      visit(node.children ?? []);
    }
  };
  visit(panel.layout);
  for (const id of ids) assert.ok(controls[id]?.label.trim(), `Missing presentation for ${id}`);
  assert.equal(controls.control_ring_options_button.label, 'Side Fn');
  for (const direction of ['up', 'down', 'left', 'right']) {
    assert.equal(controls[`selector.${direction}`].icon, direction);
    assert.ok(icons[direction].lines.length);
  }
});
