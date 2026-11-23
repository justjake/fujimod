import { Validator } from '@cfworker/json-schema';
import type { JsonObject, JsonValue, Schema } from './schema';

/** Problems by JSON pointer (`/save_data/date_folders`), one message each,
 * worded for people. Top-level sections the schema does not describe are not
 * problems: the camera keeps them for mods this schema does not cover. */
export function problems(value: JsonValue, schema: Schema): Map<string, string> {
  const known = schema.properties ?? {};
  const checked =
    schema.type === 'object' && typeof value === 'object' && value && !Array.isArray(value)
      ? Object.fromEntries(Object.entries(value as JsonObject).filter(([key]) => key in known))
      : value;
  const result = new Validator(schema as object, '7', false).validate(checked);
  const found = new Map<string, string>();
  for (const error of result.errors) {
    const pointer = error.instanceLocation.replace(/^#/, '');
    const node = schemaAt(schema, pointer);
    const message = describe(error.keyword, node, error.error);
    if (!message) continue;
    // The deepest problem is the useful one; parents only repeat it.
    for (const key of [...found.keys()])
      if (pointer.startsWith(key + '/') || key === '') found.delete(key);
    if (![...found.keys()].some((key) => key.startsWith(pointer + '/')))
      found.set(pointer, message);
  }
  return found;
}

function schemaAt(schema: Schema, pointer: string): Schema | undefined {
  let node: Schema | undefined = schema;
  for (const part of pointer.split('/').slice(1)) {
    const key = part.replace(/~1/g, '/').replace(/~0/g, '~');
    node =
      node?.properties?.[key] ??
      (typeof node?.additionalProperties === 'object' ? node.additionalProperties : undefined) ??
      node?.items;
  }
  return node;
}

function describe(keyword: string, node: Schema | undefined, fallback: string) {
  switch (keyword) {
    case 'minimum':
    case 'maximum':
      return `Choose a value from ${node?.minimum} to ${node?.maximum}.`;
    case 'multipleOf':
      return `Use steps of ${node?.multipleOf}.`;
    case 'enum':
      return 'Choose one of the listed values.';
    case 'maxLength':
      return `Use at most ${node?.maxLength} characters.`;
    case 'minLength':
      return node?.minLength === 1
        ? 'Enter a value.'
        : `Use at least ${node?.minLength} characters.`;
    case 'maxItems':
      return `Use at most ${node?.maxItems} entries.`;
    case 'type':
      return 'This value has the wrong kind.';
    case 'additionalProperties':
      return fallback.replace(/^Property "([^"]+)".*$/, 'The camera does not know "$1".');
    case 'required':
      return 'A required value is missing.';
    case 'properties':
    case 'items':
    case 'additionalItems':
      return undefined;
    default:
      return fallback;
  }
}
