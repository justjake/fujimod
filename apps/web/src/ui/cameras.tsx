import { For, Show, createEffect, createSignal, onCleanup, onSettled } from 'solid-js';
import { phases, started, stepPassed, type FirmwareProgress, type Phase } from './camera-progress';
import './cameras.css';
export interface Generation {
  name: string;
  years: string;
  models: string[];
}
/** A contributor and the camera bodies they own. */
export interface Owner {
  name: string;
  github: string;
  bodies: string[];
}
/** Placeholder until each camera has its own page. */
const CONTRIBUTE = 'dev-tools/';
/** Letters whose names start with a vowel sound take "an": an X-E2, an F… */
const AN = /^[AEFHILMNORSX]/;
function help(model: string, phase: Phase) {
  if (phase.id !== 'camera' || phase.passed === phase.steps.length) return undefined;
  if (phase.passed) return `Help test on your ${model}`;
  return `Have ${AN.test(model) ? 'an' : 'a'} ${model}? Help test it`;
}
/** The open row, shared across the table so only one detail panel is visible. */
interface Tips {
  open: () => string | undefined;
  show: (key: string | undefined) => void;
}
function createTips(): Tips {
  const [open, show] = createSignal<string>();
  onSettled(() => {
    const outside = (event: PointerEvent) => {
      if (!(event.target instanceof Element && event.target.closest('#camera-table .progress-bar')))
        show(undefined);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') show(undefined);
    };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('pointerdown', outside);
      document.removeEventListener('keydown', escape);
    };
  });
  return { open, show: (key) => show(() => key) };
}
function ProgressBar(props: { progress: FirmwareProgress; owners: Owner[]; tips: Tips }) {
  const key = () => props.progress.model;
  const groups = () => phases(props.progress);
  const total = () => groups().reduce((sum, group) => sum + group.steps.length, 0);
  const passed = () => groups().reduce((sum, group) => sum + group.passed, 0);
  const value = () => `${passed()} of ${total()}`;
  // A mouse opens the tip by hovering; a tap toggles it. Focus from a tap is not a keyboard visit.
  let pressed = false,
    wasOpen = false;
  let tip: HTMLDivElement | undefined;
  const place = () => {
    if (!tip) return;
    tip.style.setProperty('--tip-shift-x', '0px');
    tip.style.setProperty('--tip-shift-y', '0px');
    const rect = tip.getBoundingClientRect();
    const margin = 8;
    let x = 0;
    let y = 0;
    if (rect.left < margin) x = margin - rect.left;
    else if (rect.right > innerWidth - margin) x = innerWidth - margin - rect.right;
    if (rect.top < margin) y = margin - rect.top;
    else if (rect.bottom > innerHeight - margin) y = innerHeight - margin - rect.bottom;
    tip.style.setProperty('--tip-shift-x', `${x}px`);
    tip.style.setProperty('--tip-shift-y', `${y}px`);
  };
  createEffect(
    () => props.tips.open() === key(),
    (open) => {
      if (!open) return;
      requestAnimationFrame(place);
      const follow = () => place();
      window.addEventListener('resize', follow);
      window.addEventListener('scroll', follow, { passive: true });
      onCleanup(() => {
        window.removeEventListener('resize', follow);
        window.removeEventListener('scroll', follow);
      });
    },
  );
  return (
    <div
      class={{ 'progress-bar': true, open: props.tips.open() === key() }}
      tabindex="0"
      onPointerEnter={(event) => event.pointerType === 'mouse' && props.tips.show(key())}
      onPointerLeave={(event) => {
        if (event.pointerType === 'mouse' && props.tips.open() === key())
          props.tips.show(undefined);
      }}
      onPointerDown={(event) => {
        pressed = event.pointerType !== 'mouse';
        wasOpen = props.tips.open() === key();
      }}
      onFocusIn={() => pressed || props.tips.show(key())}
      onFocusOut={(event) => {
        const next = event.relatedTarget;
        if (
          !(next instanceof Node && event.currentTarget.contains(next)) &&
          props.tips.open() === key()
        )
          props.tips.show(undefined);
      }}
      onClick={(event) => {
        if (!pressed) return;
        pressed = false;
        if (event.target instanceof Element && event.target.closest('.tip')) return;
        props.tips.show(wasOpen ? undefined : key());
      }}
    >
      <div
        class="track"
        role="progressbar"
        aria-label={`${key()} progress`}
        aria-valuemin={0}
        aria-valuemax={total() || 1}
        aria-valuenow={passed()}
        aria-valuetext={value()}
      >
        <For each={groups()}>
          {(group) => (
            <div
              class="track-group"
              style={{
                '--outcomes': String(group.steps.length),
                '--p': `${(group.passed / group.steps.length) * 100}%`,
                '--c': `var(--phase-${group.id})`,
              }}
            >
              <div class="fill" />
            </div>
          )}
        </For>
      </div>
      <div
        class="tip"
        ref={(element) => {
          tip = element;
        }}
      >
        <div class="tip-scroll">
          <div class="tip-value">{value()}</div>
          <For each={groups()}>
            {(group) => (
              <div class="tip-group" style={{ '--c': `var(--phase-${group.id})` }}>
                <div class="tip-label">
                  {group.label}
                  <span class="step-detail">
                    {group.passed} of {group.steps.length}
                  </span>
                </div>
                <ul>
                  <For each={group.steps}>
                    {(step) => (
                      <li class={{ todo: !stepPassed(step) }}>
                        {step.label}
                        <Show
                          when={step.result === 'stale' || step.result === 'fail' || step.detail}
                        >
                          <span class="step-detail">
                            {step.result === 'stale'
                              ? 'Stale'
                              : step.result === 'fail'
                                ? 'Failed'
                                : step.detail}
                          </span>
                        </Show>
                      </li>
                    )}
                  </For>
                </ul>
                <Show when={group.id === 'camera' && props.owners.length}>
                  <div class="owners">
                    Owned by{' '}
                    <For each={props.owners}>
                      {(owner, index) => (
                        <>
                          {index() ? ', ' : ''}
                          <a href={`https://github.com/${owner.github}`}>@{owner.github}</a>
                        </>
                      )}
                    </For>
                  </div>
                </Show>
                <Show when={help(key(), group)}>
                  {(text) => (
                    <a class="help" href={CONTRIBUTE}>
                      {text()}
                    </a>
                  )}
                </Show>
              </div>
            )}
          </For>
        </div>
      </div>
    </div>
  );
}
function ProgressRow(props: {
  progress: FirmwareProgress;
  owners: Owner[];
  tips: Tips;
  released: boolean;
}) {
  return (
    <div class={{ 'progress-row': true, owned: props.owners.length > 0 }}>
      <div class="progress-heading">
        <span class="name">{props.progress.model}</span>
        <span class="version">{props.progress.version}</span>
        <Show when={props.released}>
          <span class="version">Released</span>
        </Show>
      </div>
      <div class="bar">
        <ProgressBar progress={props.progress} owners={props.owners} tips={props.tips} />
      </div>
    </div>
  );
}
function GenerationGroup(props: {
  group: Generation;
  progress: Record<string, FirmwareProgress>;
  owners: ReadonlyMap<string, Owner[]>;
  tips: Tips;
  released?: ReadonlySet<string>;
}) {
  const rows = () =>
    props.group.models.flatMap((model) => {
      const progress = props.progress[model];
      return progress && started(progress) ? [progress] : [];
    });
  const rest = () =>
    props.group.models.filter((model) => !rows().some((row) => row.model === model));
  return (
    <div class="gen">
      <h4>
        {props.group.name}
        <span>{props.group.years}</span>
      </h4>
      <For each={rows()}>
        {(progress) => (
          <ProgressRow
            progress={progress}
            owners={props.owners.get(progress.model) ?? []}
            tips={props.tips}
            released={props.released?.has(progress.firmware_id) ?? false}
          />
        )}
      </For>
      <Show when={rest().length}>
        <div class="rest">
          <b>Not started</b>
          <For each={rest()}>
            {(model, index) => (
              <>
                {index() ? ' · ' : ''}
                <span class={{ owned: props.owners.has(model) }}>{model}</span>
              </>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
export function CameraTable(props: {
  columns: Generation[][];
  progress: Record<string, FirmwareProgress>;
  owners: Owner[];
  released?: ReadonlySet<string>;
}) {
  const tips = createTips();
  const owners = () => {
    const result = new Map<string, Owner[]>();
    for (const owner of props.owners)
      for (const body of owner.bodies) result.set(body, [...(result.get(body) ?? []), owner]);
    return result;
  };
  const legend = () => {
    const tracks = new Map<Phase['id'], string>();
    for (const progress of Object.values(props.progress))
      for (const track of progress.tracks) tracks.set(track.phase, track.title);
    return [...tracks];
  };
  const listed = () => new Set(props.columns.flat().flatMap((group) => group.models));
  const unlisted = () =>
    Object.values(props.progress)
      .filter((progress) => !listed().has(progress.model) && started(progress))
      .map((progress) => progress.model)
      .sort();
  return (
    <section class="gens" id="camera-table" aria-labelledby="cameras-heading">
      <div class="cameras-head">
        <h2 class="section-heading" id="cameras-heading">
          Cameras
        </h2>
        <ul class="legend" aria-label="Legend">
          <For each={legend()}>
            {([id, title]) => <li style={{ '--c': `var(--phase-${id})` }}>{title}</li>}
          </For>
          <li class="owned">Owned by a contributor</li>
        </ul>
      </div>
      <For each={props.columns}>
        {(groups) => (
          <div class="col">
            <For each={groups}>
              {(group) => (
                <GenerationGroup
                  group={group}
                  progress={props.progress}
                  owners={owners()}
                  tips={tips}
                  released={props.released}
                />
              )}
            </For>
          </div>
        )}
      </For>
      <Show when={unlisted().length}>
        <div class="col">
          <GenerationGroup
            group={{ name: 'Other cameras', years: '', models: unlisted() }}
            progress={props.progress}
            owners={owners()}
            tips={tips}
            released={props.released}
          />
        </div>
      </Show>
    </section>
  );
}
