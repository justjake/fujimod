import { For, Show, createSignal } from 'solid-js';
import {
  defaultOf,
  enumTitle,
  filled,
  groupsOf,
  titleOf,
  type JsonObject,
  type JsonValue,
  type Schema,
} from './schema';

export type FieldContext = {
  /** Problems by JSON pointer. */
  problems: () => ReadonlyMap<string, string>;
  /** Names of documents an `x-reference` may name, by document id. */
  references: (id: string) => readonly string[] | undefined;
  disabled: () => boolean;
};

type FieldProps = {
  key: string;
  schema: Schema;
  value: JsonValue;
  pointer: string;
  set: (value: JsonValue) => void;
  context: FieldContext;
};

const isObject = (value: JsonValue): value is JsonObject =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

/** A value as text: strings as they are, anything else as JSON. */
const plain = (value: JsonValue) => (typeof value === 'string' ? value : JSON.stringify(value));

const pointerTo = (parent: string, key: string | number) =>
  `${parent}/${String(key).replace(/~/g, '~0').replace(/\//g, '~1')}`;

/** A number as the camera shows it: its title, or signed when it is an
 * adjustment around zero, with its unit. */
function numberText(schema: Schema, value: number) {
  const title = schema['x-enum-titles']?.[String(value)];
  if (title) return title;
  const text = Number.isInteger(value) ? String(value) : value.toFixed(1).replace(/\.0$/, '');
  const adjustment = (schema.minimum ?? 0) < 0 && !schema.examples;
  const signed = adjustment && value > 0 ? '+' + text : text;
  return schema['x-unit'] ? `${signed} ${schema['x-unit']}` : signed;
}

/** The values a number may take: its examples, or every value of a short
 * range; undefined when it is free. */
function steps(schema: Schema) {
  if (schema.examples?.every((example) => typeof example === 'number'))
    return schema.examples as number[];
  const { minimum, maximum } = schema;
  if (minimum === undefined || maximum === undefined) return undefined;
  const step = schema.multipleOf ?? 1;
  const count = Math.round((maximum - minimum) / step) + 1;
  if (count > 41) return undefined;
  return Array.from(
    { length: count },
    (_, index) => Math.round((minimum + index * step) * 1e6) / 1e6,
  );
}

function Problem(props: { context: FieldContext; pointer: string }) {
  return (
    <Show when={props.context.problems().get(props.pointer)}>
      {(message) => <p class="doc-problem">{message()}</p>}
    </Show>
  );
}

/** The control for one value: a switch, a list, a number or text. */
function Control(props: FieldProps & { id: string }) {
  const schema = () => props.schema;
  const disabled = () => props.context.disabled() || !!schema().readOnly;
  const unset = () => (schema().default === undefined ? [{ value: null, label: '—' }] : []);
  const choices = (): { value: JsonValue; label: string }[] | undefined => {
    const node = schema();
    if (node.enum)
      return [...unset(), ...node.enum.map((value) => ({ value, label: enumTitle(node, value) }))];
    if (node['x-reference']) {
      const names = props.context.references(node['x-reference']);
      if (!names) return undefined;
      const all = [
        ...new Set(['', ...names, ...(typeof props.value === 'string' ? [props.value] : [])]),
      ];
      return all.map((value) => ({ value, label: value || '—' }));
    }
    if (node.type === 'integer' || node.type === 'number') {
      const values = steps(node);
      return (
        values && [
          ...unset(),
          ...values.map((value) => ({ value, label: numberText(node, value) })),
        ]
      );
    }
    return undefined;
  };
  return (
    <Show
      when={schema().type !== 'boolean'}
      fallback={
        <input
          id={props.id}
          class="doc-switch"
          type="checkbox"
          role="switch"
          checked={props.value === true}
          disabled={disabled()}
          onChange={(event) => props.set(event.currentTarget.checked)}
        />
      }
    >
      <Show
        when={choices()}
        fallback={
          <Show
            when={schema().type === 'integer' || schema().type === 'number'}
            fallback={
              <Show
                when={(schema().maxLength ?? 0) > 255}
                fallback={
                  <input
                    id={props.id}
                    type="text"
                    value={typeof props.value === 'string' ? props.value : ''}
                    maxlength={schema().maxLength}
                    list={schema().examples ? props.id + '-examples' : undefined}
                    disabled={disabled()}
                    spellcheck={false}
                    onInput={(event) => props.set(event.currentTarget.value)}
                  />
                }
              >
                <textarea
                  id={props.id}
                  rows={3}
                  value={typeof props.value === 'string' ? props.value : ''}
                  maxlength={schema().maxLength}
                  disabled={disabled()}
                  onInput={(event) => props.set(event.currentTarget.value)}
                />
              </Show>
            }
          >
            <input
              id={props.id}
              type="number"
              inputmode="decimal"
              value={typeof props.value === 'number' ? props.value : ''}
              min={schema().minimum}
              max={schema().maximum}
              step={schema().multipleOf ?? (schema().type === 'integer' ? 1 : 'any')}
              disabled={disabled()}
              onInput={(event) => {
                const number = event.currentTarget.valueAsNumber;
                if (!Number.isNaN(number)) props.set(number);
              }}
            />
          </Show>
        }
      >
        {(list) => (
          <select
            id={props.id}
            disabled={disabled()}
            onChange={(event) => {
              const chosen = list()[event.currentTarget.selectedIndex];
              if (chosen) props.set(chosen.value);
            }}
          >
            <Show when={!list().some((choice) => choice.value === props.value)}>
              <option selected>
                {typeof props.value === 'number'
                  ? numberText(schema(), props.value)
                  : plain(props.value)}
              </option>
            </Show>
            <For each={list()}>
              {(choice) => <option selected={choice.value === props.value}>{choice.label}</option>}
            </For>
          </select>
        )}
      </Show>
      <Show when={schema().examples}>
        {(examples) => (
          <datalist id={props.id + '-examples'}>
            <For each={examples()}>{(example) => <option value={plain(example)} />}</For>
          </datalist>
        )}
      </Show>
    </Show>
  );
}

/** One row: the setting's title and its control. */
function Row(props: FieldProps) {
  const id = () => 'doc' + props.pointer.replace(/[^a-zA-Z0-9]+/g, '-');
  return (
    <div class="doc-row">
      <label for={id()}>{titleOf(props.key, props.schema)}</label>
      <Control {...props} id={id()} />
      <Problem context={props.context} pointer={props.pointer} />
    </div>
  );
}

/** A value of any schema: rows for plain values, sections for objects,
 * lists for arrays and maps. */
export function Field(props: FieldProps & { depth: number }) {
  const schema = () => props.schema;
  const context: FieldContext = {
    problems: () => props.context.problems(),
    references: (id) => props.context.references(id),
    disabled: () => props.context.disabled() || !!props.schema.readOnly,
  };
  return (
    <Show
      when={schema().type === 'object' || schema().type === 'array'}
      fallback={<Row {...props} />}
    >
      <section class="doc-section" data-depth={props.depth}>
        <Show when={props.key}>
          <h2 class="doc-section-title">{titleOf(props.key, schema())}</h2>
        </Show>
        <Problem context={props.context} pointer={props.pointer} />
        <Show
          when={schema().type === 'object'}
          fallback={<ListField {...props} context={context} />}
        >
          <Show when={schema().properties} fallback={<MapField {...props} context={context} />}>
            <ObjectFields {...props} context={context} />
          </Show>
        </Show>
      </section>
    </Show>
  );
}

/** An object's properties, in its groups. Plain values in a group share one
 * card; nested objects follow as their own sections. */
export function ObjectFields(props: FieldProps & { depth: number }) {
  const value = () => (isObject(props.value) ? props.value : {});
  const set = (key: string, child: JsonValue) => props.set({ ...value(), [key]: child });
  const plain = (key: string) => {
    const type = props.schema.properties?.[key]?.type;
    return type !== 'object' && type !== 'array';
  };
  return (
    <For each={groupsOf(props.schema)}>
      {(group) => (
        <>
          <Show when={group.title && !group.keys.some(plain)}>
            <h2 class="doc-group-heading">{group.title}</h2>
          </Show>
          <Show when={group.keys.some(plain)}>
            <div class="doc-card">
              <Show when={group.title}>
                <h3 class="doc-group-title">{group.title}</h3>
              </Show>
              <For each={group.keys.filter(plain)}>
                {(key) => (
                  <Row
                    key={key}
                    schema={props.schema.properties![key]!}
                    value={value()[key] ?? defaultOf(props.schema.properties![key]!)}
                    pointer={pointerTo(props.pointer, key)}
                    set={(child) => set(key, child)}
                    context={props.context}
                  />
                )}
              </For>
            </div>
          </Show>
          <For each={group.keys.filter((key) => !plain(key))}>
            {(key) => (
              <Field
                key={key}
                schema={props.schema.properties![key]!}
                value={value()[key] ?? defaultOf(props.schema.properties![key]!)}
                pointer={pointerTo(props.pointer, key)}
                set={(child) => set(key, child)}
                context={props.context}
                depth={props.depth + 1}
              />
            )}
          </For>
        </>
      )}
    </For>
  );
}

/** An array: each entry in turn, with Remove, and Add while there is room. */
function ListField(props: FieldProps & { depth: number }) {
  const items = () => (Array.isArray(props.value) ? props.value : []);
  const item = () => props.schema.items ?? {};
  const room = () => items().length < (props.schema.maxItems ?? Infinity);
  return (
    <div class="doc-card">
      <For each={items()}>
        {(entry, index) => (
          <div class="doc-entry">
            <Field
              key=""
              schema={item()}
              value={entry}
              pointer={pointerTo(props.pointer, index())}
              set={(child) => props.set(items().map((old, at) => (at === index() ? child : old)))}
              context={props.context}
              depth={props.depth + 1}
            />
            <button
              type="button"
              class="doc-remove"
              disabled={props.context.disabled()}
              onClick={() => props.set(items().filter((_, at) => at !== index()))}
            >
              Remove
            </button>
          </div>
        )}
      </For>
      <Show when={room()}>
        <button
          type="button"
          class="doc-add"
          disabled={props.context.disabled()}
          onClick={() => props.set([...items(), filled(undefined, item())])}
        >
          Add
        </button>
      </Show>
    </div>
  );
}

/** An object whose keys are chosen by the user: each entry, with Remove,
 * and a name field to add one. */
function MapField(props: FieldProps & { depth: number }) {
  const [name, setName] = createSignal('');
  const value = () => (isObject(props.value) ? props.value : {});
  const entry = () =>
    typeof props.schema.additionalProperties === 'object' ? props.schema.additionalProperties : {};
  const add = () => {
    const key = name().trim();
    if (!key || key in value()) return;
    props.set({ ...value(), [key]: filled(undefined, entry()) });
    setName('');
  };
  return (
    <div class="doc-card">
      <For each={Object.keys(value())}>
        {(key) => (
          <div class="doc-entry">
            <Field
              key={key}
              schema={{ ...entry(), title: key }}
              value={value()[key]!}
              pointer={pointerTo(props.pointer, key)}
              set={(child) => props.set({ ...value(), [key]: child })}
              context={props.context}
              depth={props.depth + 1}
            />
            <button
              type="button"
              class="doc-remove"
              disabled={props.context.disabled()}
              onClick={() => {
                const { [key]: _, ...rest } = value();
                props.set(rest);
              }}
            >
              Remove
            </button>
          </div>
        )}
      </For>
      <form
        class="doc-add-key"
        onSubmit={(event) => {
          event.preventDefault();
          add();
        }}
      >
        <input
          type="text"
          value={name()}
          aria-label="Name"
          placeholder="Name"
          disabled={props.context.disabled()}
          onInput={(event) => setName(event.currentTarget.value)}
        />
        <button type="submit" class="doc-add" disabled={props.context.disabled() || !name().trim()}>
          Add
        </button>
      </form>
    </div>
  );
}
