import { createSignal, type Accessor, type Setter } from 'solid-js';
import type { ControlSession } from './controls';
import {
  exportDevicePanel,
  importDevicePanel,
  nodeAt,
  removeAt,
  replaceAt,
  type ControlNode,
  type DevicePanel,
  type NodePath,
} from './layout';

/** Where an editor finds its live panel and reports layout changes. */
export interface LayoutOwner {
  readonly runtime?: { readonly panel: ControlSession };
  readonly onLayoutChange?: (panel: DevicePanel) => void | Promise<void>;
}

/** Editing state for one live control panel. The renderer only reads it. */
export class LayoutEditor {
  readonly editing: Accessor<boolean>;
  readonly selected: Accessor<NodePath | undefined>;
  private readonly setEditing: Setter<boolean>;
  private readonly setSelected: Setter<NodePath | undefined>;

  constructor(private readonly owner: LayoutOwner) {
    [this.editing, this.setEditing] = createSignal(false);
    [this.selected, this.setSelected] = createSignal<NodePath | undefined>(undefined, {
      equals: (a, b) => a?.join('.') === b?.join('.'),
    });
  }

  private session(): ControlSession | undefined {
    return this.owner.runtime?.panel;
  }

  private panel(): DevicePanel | undefined {
    return this.session()?.read().data;
  }

  begin() {
    this.setEditing(true);
  }

  end() {
    this.setSelected(undefined);
    this.setEditing(false);
  }

  select(path: NodePath | undefined) {
    if (!this.editing()) return;
    const panel = this.panel();
    this.setSelected(path && panel && nodeAt(panel, path) ? path : undefined);
  }

  selectedNode(): ControlNode | undefined {
    const panel = this.panel();
    const path = this.selected();
    return panel && path ? nodeAt(panel, path) : undefined;
  }

  isSelected(path: NodePath) {
    return this.selected()?.join('.') === path.join('.');
  }

  update(path: NodePath, patch: Partial<ControlNode>) {
    const panel = this.panel();
    const node = panel && nodeAt(panel, path);
    if (node) this.apply(replaceAt(panel, path, { ...node, ...patch }));
  }

  /** Bind a D-pad or command control slot (up, left, center, …) to an input. */
  bind(path: NodePath, slot: string, inputId: string) {
    const panel = this.panel();
    const node = panel && nodeAt(panel, path);
    if (node) this.update(path, { bindings: { ...node.bindings, [slot]: inputId } });
  }

  remove(path: NodePath) {
    const panel = this.panel();
    if (!panel) return;
    this.setSelected(undefined);
    this.apply(removeAt(panel, path));
  }

  /** Replace the panel with an imported layout; returns ids the emulator does not have. */
  importJson(text: string): string[] {
    const session = this.session();
    const panel = this.panel();
    if (!session || !panel) return [];
    const imported = importDevicePanel(text, panel.body, [...session.inputs.values()]);
    this.setSelected(undefined);
    this.apply(imported.panel);
    return imported.unknownInputIds;
  }

  exportJson(): string | undefined {
    const panel = this.panel();
    return panel && exportDevicePanel(panel);
  }

  private apply(panel: DevicePanel) {
    this.session()?.replacePanel(panel);
    void this.owner.onLayoutChange?.(panel);
  }
}
