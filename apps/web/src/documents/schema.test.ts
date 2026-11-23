import { expect, test } from 'vite-plus/test';
import { documentText, filled, groupsOf, pruned, type JsonObject, type Schema } from './schema.ts';
import { documentSchemas, recipeSchema, settingsSchema } from './schemas.ts';
import { problems } from './validation.ts';

const schema: Schema = {
  type: 'object',
  'x-groups': [{ title: 'ONE', properties: ['b'] }],
  properties: {
    a: { type: 'integer', minimum: -2, maximum: 2, default: 0 },
    b: { type: 'string', enum: ['off', 'on'], default: 'off' },
    nested: { type: 'object', properties: { c: { type: 'boolean', default: false } } },
    list: { type: 'array', maxItems: 2, items: { type: 'integer', default: 1 } },
    map: {
      type: 'object',
      additionalProperties: {
        type: 'object',
        properties: { d: { type: 'integer', minimum: 0, maximum: 9 } },
      },
    },
  },
};

test('filled gives every property its default and keeps unknown keys', () => {
  expect(filled({ a: 1, other: 'kept' }, schema)).toEqual({
    a: 1,
    b: 'off',
    nested: { c: false },
    list: [],
    map: {},
    other: 'kept',
  });
});

test('pruned leaves out defaults, empty objects and unset values', () => {
  const value = filled({ a: 0, b: 'on', map: { exr2: { d: null } } }, schema);
  expect(pruned(value, schema)).toEqual({ b: 'on', map: { exr2: {} } });
  expect(documentText(filled(undefined, schema), schema)).toBe('{}\n');
});

test('documentText keeps schema order, two-space indentation and a final newline', () => {
  const value = filled({ list: [3], b: 'on', a: -1 }, schema);
  expect(documentText(value, schema)).toBe(
    '{\n  "a": -1,\n  "b": "on",\n  "list": [\n    3\n  ]\n}\n',
  );
});

test('groups come in order, then the properties no group names', () => {
  expect(groupsOf(schema)).toEqual([
    { title: 'ONE', keys: ['b'] },
    { keys: ['a', 'nested', 'list', 'map'] },
  ]);
});

test('problems are worded per field at the deepest pointer', () => {
  const found = problems({ a: 5, list: [1, 2, 3], map: { x: { d: 12 } } }, schema);
  expect(Object.fromEntries(found)).toEqual({
    '/a': 'Choose a value from -2 to 2.',
    '/list': 'Use at most 2 entries.',
    '/map/x/d': 'Choose a value from 0 to 9.',
  });
});

test('sections the schema does not describe are kept and not problems', () => {
  expect(problems({ future_mod: { x: 1 } }, settingsSchema).size).toBe(0);
});

const objects = (node: Schema): Schema[] => [
  node,
  ...Object.values(node.properties ?? {}).flatMap(objects),
  ...(typeof node.additionalProperties === 'object' ? objects(node.additionalProperties) : []),
  ...(node.items ? objects(node.items) : []),
];

test('every document schema names its location, title and groups correctly', () => {
  const ids = new Set(documentSchemas.map((document) => document['x-document']?.id));
  expect(ids).toEqual(new Set(['settings', 'recipe']));
  for (const document of documentSchemas) {
    expect(document.title).toBeTruthy();
    for (const node of objects(document)) {
      for (const group of node['x-groups'] ?? [])
        for (const key of group.properties)
          expect(node.properties, `${group.title}: ${key}`).toHaveProperty(key);
      if (node['x-reference']) expect(ids).toContain(node['x-reference']);
      if (node['x-locked-by']) expect(node.properties?.[node['x-locked-by']]?.type).toBe('boolean');
    }
  }
});

test('a camera recipe round-trips without changing its values', () => {
  const recipe: JsonObject = {
    protected: true,
    v: 1,
    name: 'High End',
    film_simulation: 'Eterna',
    dynamic_range: 'dr400',
    white_balance_red: 4,
    highlight: 1,
    iso: -1,
    auto_iso1: { base: 640, maximum: 1600, min_shutter_denominator: 60 },
    grain_roughness: 'strong',
  };
  const text = documentText(filled(recipe, recipeSchema), recipeSchema);
  expect(problems(JSON.parse(text), recipeSchema).size).toBe(0);
  const { v: _, ...changed } = recipe;
  // ISO and AUTO ISO have no default: what the file names is kept, at any value.
  expect(JSON.parse(text)).toEqual(changed);
});

test('settings a recipe leaves out stay out', () => {
  const text = documentText(
    filled({ name: 'Plain', film_simulation: 'Velvia' }, recipeSchema),
    recipeSchema,
  );
  expect(JSON.parse(text)).toEqual({ name: 'Plain', film_simulation: 'Velvia' });
});
