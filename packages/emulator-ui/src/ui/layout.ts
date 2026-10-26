import { isDetentSound, type DetentSound } from '../feedback/detent-sound';
import type { BootMode, ControlIcon } from 'fujimod';
import type { BootInput, ControlInput } from 'fujimod/emulator';

export const DEVICE_PANEL_SCHEMA = 1 as const;

const controlKinds = [
  'control',
  'grid',
  'row',
  'column',
  'divider',
  'space',
  'command',
  'rocker',
  'dpad',
] as const;
export const CONTROL_VARIANTS = ['dial', 'ticks', 'segment', 'shutter', 'key', 'record'] as const;

export interface ControlNode {
  kind: (typeof controlKinds)[number];
  className?: string;
  id?: string;
  variant?: (typeof CONTROL_VARIANTS)[number];
  caption?: string;
  operation?: 'power';
  face?: string[];
  hint?: string;
  icon?: ControlIcon;
  labels?: Record<string, string>;
  children?: ControlNode[];
  bindings?: Record<string, string>;
}
export interface PowerCombo {
  label: string;
  modes: BootMode[];
  inputs: BootInput[];
}
export interface DevicePanel {
  schema: typeof DEVICE_PANEL_SCHEMA;
  body: string;
  /** Acoustic character of each physical dial, shared by panel and model. */
  detentSounds?: Record<string, DetentSound>;
  layout: ControlNode[];
  combos: PowerCombo[];
}

export interface DevicePanelImport {
  panel: DevicePanel;
  unknownInputIds: string[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isOneOf<const Values extends readonly string[]>(
  value: unknown,
  values: Values,
): value is Values[number] {
  return typeof value === 'string' && values.some((candidate) => candidate === value);
}

function isStringRecord(value: unknown): value is Record<string, string> {
  return isRecord(value) && Object.values(value).every((item) => typeof item === 'string');
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === 'string');
}

function isOptionalString(value: unknown): boolean {
  return value === undefined || typeof value === 'string';
}

function isOptionalIdentifier(value: unknown): boolean {
  return value === undefined || (typeof value === 'string' && value.length > 0);
}

function isIdentifier(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0;
}

function isControlNode(value: unknown): value is ControlNode {
  if (!isRecord(value) || !isOneOf(value.kind, controlKinds)) return false;
  return (
    isOptionalString(value.className) &&
    isOptionalIdentifier(value.id) &&
    isOptionalString(value.caption) &&
    isOptionalString(value.hint) &&
    (value.icon === undefined || isOneOf(value.icon, ['playback', 'delete', 'record', 'flash'])) &&
    (value.variant === undefined || isOneOf(value.variant, CONTROL_VARIANTS)) &&
    (value.operation === undefined || value.operation === 'power') &&
    (value.face === undefined || isStringArray(value.face)) &&
    (value.labels === undefined || isStringRecord(value.labels)) &&
    (value.bindings === undefined || isStringRecord(value.bindings)) &&
    (value.children === undefined ||
      (Array.isArray(value.children) && value.children.every(isControlNode)))
  );
}

function isBootInput(value: unknown): value is BootInput {
  return (
    isRecord(value) &&
    isIdentifier(value.id) &&
    isOptionalString(value.stage) &&
    isOptionalString(value.position)
  );
}

function isPowerCombo(value: unknown): value is PowerCombo {
  return (
    isRecord(value) &&
    typeof value.label === 'string' &&
    Array.isArray(value.modes) &&
    value.modes.every(
      (mode) => mode === 'sd' || mode === 'firmware_update' || mode === 'service',
    ) &&
    Array.isArray(value.inputs) &&
    value.inputs.every(isBootInput)
  );
}

function isDetentSoundRecord(value: unknown): value is Record<string, DetentSound> {
  return isRecord(value) && Object.values(value).every(isDetentSound);
}

function isDevicePanel(value: unknown): value is DevicePanel {
  return (
    isRecord(value) &&
    value.schema === DEVICE_PANEL_SCHEMA &&
    typeof value.body === 'string' &&
    value.body.length > 0 &&
    Array.isArray(value.layout) &&
    value.layout.every(isControlNode) &&
    Array.isArray(value.combos) &&
    value.combos.every(isPowerCombo) &&
    (value.detentSounds === undefined || isDetentSoundRecord(value.detentSounds))
  );
}

function boundInputIds(panel: DevicePanel): string[] {
  const ids = new Set<string>();
  const visit = (nodes: ControlNode[]) => {
    for (const node of nodes) {
      if (node.id) ids.add(node.id);
      for (const id of Object.values(node.bindings ?? {})) ids.add(id);
      visit(node.children ?? []);
    }
  };
  visit(panel.layout);
  for (const combo of panel.combos) for (const input of combo.inputs) ids.add(input.id);
  return [...ids];
}

export function unknownDevicePanelInputIds(
  panel: DevicePanel,
  inputs: readonly Pick<ControlInput, 'id'>[],
): string[] {
  const knownIds = new Set(inputs.map((input) => input.id));
  return boundInputIds(panel).filter((id) => !knownIds.has(id));
}

export function exportDevicePanel(panel: DevicePanel): string {
  if (!isDevicePanel(panel)) throw Error('Cannot export an invalid device panel.');
  return `${JSON.stringify(panel, null, 2)}\n`;
}

export function importDevicePanel(
  json: string,
  expectedBody: string,
  inputs: readonly Pick<ControlInput, 'id'>[],
): DevicePanelImport {
  const value: unknown = JSON.parse(json);
  if (!isDevicePanel(value))
    throw Error('Invalid device panel JSON or unsupported schema version.');
  if (value.body !== expectedBody)
    throw Error(`Device panel body mismatch: expected ${expectedBody}, received ${value.body}.`);
  return {
    panel: value,
    unknownInputIds: unknownDevicePanelInputIds(value, inputs),
  };
}

/** A node's position in a panel: child indexes from the root layout. */
export type NodePath = readonly number[];

export function nodeAt(panel: DevicePanel, path: NodePath): ControlNode | undefined {
  let nodes = panel.layout;
  let node: ControlNode | undefined;
  for (const index of path) {
    node = nodes[index];
    if (!node) return undefined;
    nodes = node.children ?? [];
  }
  return node;
}

function rewrite(
  nodes: ControlNode[],
  path: NodePath,
  change: (node: ControlNode) => ControlNode | undefined,
): ControlNode[] {
  const [index, ...rest] = path;
  return nodes.flatMap((node, i) => {
    if (i !== index) return [node];
    if (!rest.length) {
      const next = change(node);
      return next ? [next] : [];
    }
    return [{ ...node, children: rewrite(node.children ?? [], rest, change) }];
  });
}

export function replaceAt(panel: DevicePanel, path: NodePath, node: ControlNode): DevicePanel {
  return { ...panel, layout: rewrite(panel.layout, path, () => node) };
}

export function removeAt(panel: DevicePanel, path: NodePath): DevicePanel {
  return { ...panel, layout: rewrite(panel.layout, path, () => undefined) };
}
