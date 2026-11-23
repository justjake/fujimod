/** The JSON Schema subset the camera's document schemas use, with the
 * annotations that drive the editor. Everything an editor page shows or does
 * comes from these keywords; nothing is specific to one document. */
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };
export type JsonObject = { [key: string]: JsonValue };

export type DocumentLocation = {
  /** Stable identity, used by `x-reference`. */
  id: string;
  /** Folder on the SD card, from its root, such as `FUJIMOD/RECIPES`. */
  directory: string;
  /** One file in the folder… */
  name?: string;
  /** …or every file with one of these extensions. */
  extensions?: readonly string[];
  /** Name of a new file; `#` is replaced by the lowest free number. */
  new_name?: string;
};

export type Group = { title: string; properties: readonly string[] };

export type Schema = {
  type?: string;
  title?: string;
  description?: string;
  default?: JsonValue;
  enum?: readonly (string | number)[];
  minimum?: number;
  maximum?: number;
  multipleOf?: number;
  minLength?: number;
  maxLength?: number;
  maxItems?: number;
  readOnly?: boolean;
  examples?: readonly JsonValue[];
  properties?: Readonly<Record<string, Schema>>;
  additionalProperties?: boolean | Schema;
  required?: readonly string[];
  items?: Schema;
  /** Labels for enum values that are not the value in capitals. */
  'x-enum-titles'?: Readonly<Record<string, string>>;
  /** Titled, ordered groups of an object's properties. */
  'x-groups'?: readonly Group[];
  /** Where documents of this schema live on the SD card. */
  'x-document'?: DocumentLocation;
  /** A string naming a document of another schema, by its `x-document` id. */
  'x-reference'?: string;
  /** A boolean property that makes the whole document read-only when true. */
  'x-locked-by'?: string;
  /** Unit shown after a number. */
  'x-unit'?: string;
};

/** A key or value as the camera would show it when no title is given. */
export function capitals(key: string) {
  return key.replace(/_/g, ' ').toUpperCase();
}

export function titleOf(key: string, schema: Schema) {
  return schema.title ?? capitals(key);
}

export function enumTitle(schema: Schema, value: string | number) {
  return (
    schema['x-enum-titles']?.[String(value)] ??
    (typeof value === 'string' ? capitals(value) : String(value))
  );
}

/** The object's properties as titled groups, in order. Properties no group
 * names follow, untitled. */
export function groupsOf(schema: Schema): { title?: string; keys: string[] }[] {
  const keys = Object.keys(schema.properties ?? {});
  const declared = schema['x-groups'] ?? [];
  const grouped = new Set(declared.flatMap((group) => group.properties));
  const rest = keys.filter((key) => !grouped.has(key));
  return [
    ...declared.map((group) => ({
      title: group.title,
      keys: group.properties.filter((key) => keys.includes(key)),
    })),
    ...(rest.length ? [{ keys: rest }] : []),
  ];
}

/** The value a missing property has: its default, or null when it has none
 * (a property that is unset until given a value). */
export function defaultOf(schema: Schema): JsonValue {
  if (schema.default !== undefined) return schema.default;
  if (schema.type === 'object') {
    const value: JsonObject = {};
    for (const [key, child] of Object.entries(schema.properties ?? {}))
      value[key] = defaultOf(child);
    return value;
  }
  if (schema.type === 'array') return [];
  return null;
}

const isObject = (value: JsonValue | undefined): value is JsonObject =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

export function equal(a: JsonValue | undefined, b: JsonValue | undefined): boolean {
  if (a === b) return true;
  if (Array.isArray(a) && Array.isArray(b))
    return a.length === b.length && a.every((item, index) => equal(item, b[index]));
  if (isObject(a) && isObject(b)) {
    const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
    return [...keys].every((key) => equal(a[key], b[key]));
  }
  return false;
}

/** The value with every missing property filled from the schema, for editing.
 * Properties the schema does not describe are kept as they are. */
export function filled(value: JsonValue | undefined, schema: Schema): JsonValue {
  if (schema.type === 'object' && schema.properties) {
    const source = isObject(value) ? value : {};
    const result: JsonObject = {};
    for (const [key, child] of Object.entries(schema.properties))
      result[key] = filled(source[key], child);
    for (const [key, child] of Object.entries(source)) if (!(key in result)) result[key] = child;
    return result;
  }
  return value === undefined ? defaultOf(schema) : value;
}

/** The value as the camera saves it: properties at their default are left
 * out, objects left empty are left out, and properties keep schema order. */
export function pruned(value: JsonValue, schema: Schema): JsonValue | undefined {
  if (schema.type === 'object' && schema.properties && isObject(value)) {
    const result: JsonObject = {};
    for (const [key, child] of Object.entries(schema.properties)) {
      if (!(key in value)) continue;
      const kept = pruned(value[key], child);
      if (kept !== undefined) result[key] = kept;
    }
    for (const [key, child] of Object.entries(value))
      if (!(key in schema.properties)) result[key] = child;
    return Object.keys(result).length ? result : undefined;
  }
  if (
    schema.type === 'object' &&
    isObject(value) &&
    typeof schema.additionalProperties === 'object'
  ) {
    const entry = schema.additionalProperties;
    const result: JsonObject = {};
    for (const [key, child] of Object.entries(value)) result[key] = pruned(child, entry) ?? {};
    return Object.keys(result).length ? result : undefined;
  }
  if (schema.type === 'array' && Array.isArray(value)) {
    if (schema.default !== undefined && equal(value, schema.default)) return undefined;
    return value.length || schema.default !== undefined ? value : undefined;
  }
  return equal(value, defaultOf(schema)) ? undefined : value;
}

/** The document's text as the camera writes it: two-space indentation, one
 * property or element per line, empty objects and arrays on one line and a
 * final newline. */
export function documentText(value: JsonValue, schema: Schema) {
  return JSON.stringify(pruned(value, schema) ?? {}, null, 2) + '\n';
}
