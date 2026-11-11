import { For, Match, Show, Switch } from 'solid-js';
import type { JSX } from '@solidjs/web';
import {
  Button,
  ChoicePopup,
  DialKey,
  Done,
  Group,
  HuePopup,
  Key,
  ProgressBar,
  Release,
  Row,
  Screen,
  Section,
  SegmentedBar,
  Spinner,
  SdCard,
  Swatch,
  SwatchPopup,
  Check,
  SWATCHES,
  type FileState,
} from './components';
import { hueColor } from './palette';

/* ---- Content ------------------------------------------------------------------ */

export const RELEASE = {
  version: '0.0.3',
  date: '2026-10-12',
  commit: 'a636fa2',
  age: 'four weeks old',
};
export const UPDATE = {
  version: '0.0.4',
  date: '2026-11-09',
  commit: 'b81c3e0',
  notes: ['Wi-Fi software updates', 'Faster menu scrolling', 'Halation keeps highlight color'],
};
export const CAMERA = { model: 'X70', modsOn: 36, mods: 36 };
export const OLDER = { version: '0.0.2', date: '2026-09-28', commit: '15c3c19' };
const NETWORK_NOTE =
  'Connection details are kept on internal storage shared with PC\u00a0AUTO\u00a0SAVE.';
const BACKUP_NOTE =
  'Includes camera settings, custom settings, and unique factory calibration data.';
const AUTOMATIC_NOTE = 'Boot FM_EXR2.BIN if present on the SD card.';
const BACKUP_FIRST = 'Back up internal storage first';
const SYSTEM_PATH = '/FUJIMOD/SYSTEM/X70-110';

/** Shown everywhere in the build overlay's order: version, commit, date. */
export type Version = { version: string; commit: string; date: string };
export type ColorRow = 'rec' | 'setup' | 'play';
export type Colors = Record<ColorRow, string>;
export const DEFAULT_COLORS: Colors = { rec: '#b91000', setup: '#0068d8', play: '#008c32' };
export const START_UP_STYLES = ['FUJIMOD', 'NONE'];
export const SHUT_DOWN_STYLES = ['CATHODE', 'NONE'];

export type SetupTarget =
  | 'wifi'
  | 'updates'
  | 'change'
  | 'backup'
  | 'welcome'
  | ColorRow
  | 'background'
  | 'startUp'
  | 'shutDown';

export type SetupPopup =
  | { kind: 'swatch'; row: ColorRow; cursor: number }
  | { kind: 'choice'; row: 'startUp' | 'shutDown'; cursor: number }
  /** `original` is restored when the popup is backed out of. */
  | { kind: 'hue'; original: number | null };

export interface SetupState {
  focus: SetupTarget;
  connected: boolean;
  lastChecked: string;
  boot: 'MANUAL' | 'AUTOMATIC';
  colors: Colors;
  hue: number | null;
  startUp: number;
  shutDown: number;
  popup?: SetupPopup;
}

export const INITIAL_SETUP: SetupState = {
  focus: 'wifi',
  connected: false,
  lastChecked: 'never',
  boot: 'MANUAL',
  colors: DEFAULT_COLORS,
  hue: null,
  startUp: 1,
  shutDown: 1,
};

/** Focus order; Check for updates is skipped until Wi-Fi connects. */
export function setupTargets(state: SetupState): SetupTarget[] {
  const all: SetupTarget[] = [
    'wifi',
    'updates',
    'change',
    'backup',
    'welcome',
    'rec',
    'setup',
    'play',
    'background',
    'startUp',
    'shutDown',
  ];
  return all.filter((target) => target !== 'updates' || state.connected);
}

/* ---- FUJIMOD SET-UP ----------------------------------------------------------- */

function BootInstructions() {
  return (
    <span class="inline-flex items-center gap-[6px]">
      Hold <Key>Fn</Key> + <DialKey /> while switching power on.
    </span>
  );
}

function SetupPopup(props: { state: SetupState; anchor: number }) {
  const popup = () => props.state.popup;
  const swatch = () => {
    const p = popup();
    return p?.kind === 'swatch' ? p : undefined;
  };
  const choice = () => {
    const p = popup();
    return p?.kind === 'choice' ? p : undefined;
  };
  const hue = () => {
    const p = popup();
    return p?.kind === 'hue' ? p : undefined;
  };
  return (
    <Switch>
      <Match when={swatch()}>
        {(p) => (
          <SwatchPopup
            anchor={props.anchor}
            cursor={p().cursor}
            current={Math.max(0, SWATCH_INDEX(props.state.colors[p().row]))}
          />
        )}
      </Match>
      <Match when={choice()}>
        {(p) => (
          <ChoicePopup
            anchor={props.anchor}
            options={p().row === 'startUp' ? START_UP_STYLES : SHUT_DOWN_STYLES}
            cursor={p().cursor}
            current={p().row === 'startUp' ? props.state.startUp : props.state.shutDown}
            description={p().row === 'startUp' ? 'PRESS ANY BUTTON TO SKIP' : undefined}
          />
        )}
      </Match>
      <Match when={hue()}>{() => <HuePopup anchor={props.anchor} hue={props.state.hue} />}</Match>
    </Switch>
  );
}

export function SetupPage(props: {
  state: SetupState;
  top?: boolean;
  onSelect?: (target: SetupTarget) => void;
}) {
  const s = () => props.state;
  const focused = (target: SetupTarget) => s().focus === target;
  const select = (target: SetupTarget) => () => props.onSelect?.(target);
  const colorRows: { row: ColorRow; label: string }[] = [
    { row: 'rec', label: 'REC COLOR' },
    { row: 'setup', label: 'SET-UP COLOR' },
    { row: 'play', label: 'PLAY COLOR' },
  ];
  const overlay = () =>
    s().popup
      ? { render: (anchor: number): JSX.Element => <SetupPopup state={s()} anchor={anchor} /> }
      : undefined;
  return (
    <Screen
      title="FUJIMOD SET-UP"
      hue={s().hue}
      accent={s().colors.setup}
      focus={`${s().focus}:${s().popup?.kind ?? ''}`}
      top={props.top}
      overlay={overlay()}
    >
      <header class="mx-margin px-[6px] pt-[16px] pb-[20px]">
        <Release version={RELEASE.version} class="text-hero" />
        <p class="mt-[10px] text-detail text-secondary">
          {RELEASE.commit}&ensp;{RELEASE.date}&ensp;•&ensp;{RELEASE.age}
        </p>
        <div class="mt-[14px] flex items-baseline justify-between text-row">
          <span>{CAMERA.model}</span>
          <span class="text-secondary">
            {CAMERA.modsOn}/{CAMERA.mods} mods
          </span>
        </div>
      </header>

      <Group>
        <Row
          label="CONNECT WI-FI"
          value={s().connected ? 'CONNECTED' : undefined}
          detail={NETWORK_NOTE}
          focused={focused('wifi')}
          onSelect={select('wifi')}
        />
        <Row
          title="CHECK FOR UPDATES"
          disabled={!s().connected}
          focused={focused('updates')}
          onSelect={select('updates')}
          detail={`Last checked ${s().lastChecked}`}
        />
      </Group>

      <Group class="mt-gap">
        <Row
          title="BOOT SETUP"
          value={s().boot}
          disclosure={false}
          detail={s().boot === 'MANUAL' ? <BootInstructions /> : AUTOMATIC_NOTE}
        />
        <Row label="CHANGE" focused={focused('change')} onSelect={select('change')} />
      </Group>

      <Group class="mt-gap">
        <Row
          label="BACK UP INTERNAL DATA"
          focused={focused('backup')}
          onSelect={select('backup')}
        />
      </Group>
      <Group class="mt-gap">
        <Row
          label="SHOW WELCOME MESSAGE"
          focused={focused('welcome')}
          onSelect={select('welcome')}
        />
      </Group>

      <Section title="Appearance" />
      <Group>
        <For each={colorRows}>
          {(entry) => (
            <Row
              label={entry.label}
              disclosure={false}
              focused={focused(entry.row)}
              onSelect={select(entry.row)}
              trailing={<Swatch color={s().colors[entry.row]} />}
            />
          )}
        </For>
        <Row
          label="BACKGROUND COLOR"
          disclosure={false}
          focused={focused('background')}
          onSelect={select('background')}
          trailing={<Swatch color={hueColor(s().hue)} />}
        />
      </Group>

      <Group class="mt-gap">
        <Row
          label="START UP STYLE"
          value={START_UP_STYLES[s().startUp]}
          focused={focused('startUp')}
          onSelect={select('startUp')}
        />
        <Row
          label="SHUT DOWN STYLE"
          value={SHUT_DOWN_STYLES[s().shutDown]}
          focused={focused('shutDown')}
          onSelect={select('shutDown')}
        />
      </Group>
    </Screen>
  );
}

export const SWATCH_INDEX = (color: string) => SWATCHES.indexOf(color);

/* ---- CHECK FOR UPDATES -------------------------------------------------------- */

export type UpdatePhase = 'checking' | 'up-to-date' | 'available' | 'downloading' | 'downloaded';

export interface UpdateState {
  phase: UpdatePhase;
  /** What slot B held when the check ran. */
  slotB: Version | null;
  files: { name: string; size: number; state: FileState }[];
  accent: string;
  hue: number | null;
}

export const RELEASE_FILES: UpdateState['files'] = [
  { name: 'fujimod.bin', size: 0.18, state: 0 },
  { name: 'firmware.patch', size: 2.45, state: 0 },
  { name: 'cert.pem', size: 0.22, state: 'present' },
  { name: 'zones.json', size: 0.25, state: 0 },
];

function mib(value: number) {
  return value.toFixed(1);
}

function StatusCard(props: { icon: JSX.Element; version: string; detail: string }) {
  return (
    <Group class="mt-[8px] overflow-hidden rounded-[16px]">
      <div class="flex flex-col items-center py-[26px]">
        {props.icon}
        <Release version={props.version} class="mt-[18px] text-card" />
        <div class="mt-[10px] text-detail text-secondary">{props.detail}</div>
      </div>
    </Group>
  );
}

function SlotLine(props: { slot: string; held: Version | null }) {
  return (
    <div class="flex items-baseline gap-[22px]">
      <span class="w-[18px] font-bold">{props.slot}</span>
      <Show when={props.held} fallback={<span class="text-secondary">none</span>}>
        {(held) => (
          <span class="flex gap-[18px]">
            <span>
              {held().version}-{held().commit}
            </span>
            <span>{held().date}</span>
          </span>
        )}
      </Show>
    </div>
  );
}

export function UpdatePage(props: { state: UpdateState; top?: boolean }) {
  const s = () => props.state;
  const slotB = () => (s().phase === 'downloaded' ? UPDATE : s().slotB);
  const sentence = () => {
    const revert = 'If the new version fails to boot, FujiMod will revert to the current version.';
    if (s().phase === 'downloaded') return revert;
    const replacing = s().slotB;
    return `Downloading this update will ${replacing ? `replace version B (${replacing.version})` : `add version B (${UPDATE.version})`}. ${revert}`;
  };
  const toDownload = () => s().files.filter((f) => f.state !== 'present');
  const done = () => toDownload().reduce((sum, f) => sum + f.size * (f.state as number), 0);
  const total = () => toDownload().reduce((sum, f) => sum + f.size, 0);
  return (
    <Screen
      title="CHECK FOR UPDATES"
      hue={s().hue}
      accent={s().accent}
      focus={s().phase}
      top={props.top}
    >
      <Show when={s().phase === 'checking'}>
        <StatusCard icon={<Spinner />} version={RELEASE.version} detail="checking" />
      </Show>
      <Show when={s().phase === 'up-to-date'}>
        <StatusCard icon={<Done />} version={RELEASE.version} detail="up to date" />
        <Group class="mt-gap">
          <Row label="CHECK AGAIN" disclosure={false} focused />
        </Group>
      </Show>
      <Show when={['available', 'downloading', 'downloaded'].includes(s().phase)}>
        <Group class="mt-[8px]">
          <div class="focal-bevel px-pad pt-[16px] pb-[18px]">
            <div class="flex items-baseline justify-between">
              <Release version={UPDATE.version} class="text-card" />
              <span class="text-detail text-secondary">
                {UPDATE.commit}&ensp;{UPDATE.date}
              </span>
            </div>
            <ul class="mt-[14px] flex flex-col gap-[6px] text-[24px] leading-[30px]">
              <For each={UPDATE.notes}>
                {(note) => (
                  <li class="flex items-center gap-[14px]">
                    <span class="size-[6px] bg-label" />
                    {note}
                  </li>
                )}
              </For>
            </ul>
          </div>
        </Group>
        <Group class="mt-gap">
          <div class="focal-bevel px-pad pt-[14px] pb-[16px] text-[26px] leading-[34px] text-secondary">
            <p>
              FujiMod stores up to two system versions in{' '}
              <span class="whitespace-nowrap">
                <SdCard /> {SYSTEM_PATH}
              </span>
            </p>
            <div class="my-[12px] text-label">
              <SlotLine slot="A" held={RELEASE} />
              <SlotLine slot="B" held={slotB()} />
            </div>
            <p>{sentence()}</p>
          </div>
        </Group>
        <div class="mt-gap">
          <Show when={s().phase === 'available'}>
            <Button label="DOWNLOAD" value={`${mib(total())} MiB`} focused />
          </Show>
          <Show when={s().phase === 'downloaded'}>
            <Button label="RESTART TO UPDATE" focused />
          </Show>
          <Show when={s().phase === 'downloading'}>
            <Group>
              <div data-focused class="focal-bevel px-pad pt-[14px] pb-[20px]">
                <div class="flex justify-between text-detail text-secondary">
                  <span>DOWNLOADING</span>
                  <span class="tabular-nums">
                    {mib(done())} / {mib(total())} MiB
                  </span>
                </div>
                <div class="mt-[12px]">
                  <SegmentedBar files={s().files} />
                </div>
              </div>
            </Group>
          </Show>
        </div>
      </Show>
    </Screen>
  );
}

/* ---- BOOT SETUP and BACK UP INTERNAL DATA -------------------------------------- */

export interface BootState {
  focus: 'MANUAL' | 'AUTOMATIC' | 'backup';
  boot: 'MANUAL' | 'AUTOMATIC';
  backedUp: boolean;
  accent: string;
  hue: number | null;
}

function Choice(props: {
  title: string;
  current: boolean;
  focused: boolean;
  disabled?: boolean;
  children: JSX.Element;
}) {
  return (
    <div
      data-focused={props.focused ? '' : undefined}
      class={`flex gap-[12px] py-[16px] pr-pad pl-[18px] ${props.focused ? 'focal-focused' : 'focal-bevel'}`}
    >
      <span class="w-[22px] pt-[4px]">
        <Show when={props.current}>
          <Check />
        </Show>
      </span>
      <div class="flex flex-col gap-[8px]">
        <span class={`text-card font-bold ${props.disabled ? 'text-disabled' : ''}`}>
          {props.title}
        </span>
        <span class="flex flex-col gap-[2px] text-detail text-secondary">{props.children}</span>
      </div>
    </div>
  );
}

export function BootPage(props: { state: BootState }) {
  const s = () => props.state;
  return (
    <Screen title="BOOT SETUP" hue={s().hue} accent={s().accent} focus={s().focus}>
      <Group class="mt-[8px]">
        <Choice title="MANUAL" current={s().boot === 'MANUAL'} focused={s().focus === 'MANUAL'}>
          <BootInstructions />
        </Choice>
      </Group>
      <Group class="mt-gap">
        <Choice
          title="AUTOMATIC"
          current={s().boot === 'AUTOMATIC'}
          focused={s().focus === 'AUTOMATIC'}
          disabled={!s().backedUp}
        >
          <span>{AUTOMATIC_NOTE}</span>
          <Show when={!s().backedUp}>
            <span class="text-warning">{BACKUP_FIRST}</span>
          </Show>
        </Choice>
      </Group>
      <Group class="mt-gap">
        <Show
          when={s().backedUp}
          fallback={<Row label="BACK UP INTERNAL DATA" focused={s().focus === 'backup'} />}
        >
          <Row label="BACKUP" value="X70_001.IMG" focused={s().focus === 'backup'} />
        </Show>
      </Group>
    </Screen>
  );
}

export function BackupPage(props: { fraction: number; accent: string; hue: number | null }) {
  const total = 128;
  return (
    <Screen title="BACK UP INTERNAL DATA" hue={props.hue} accent={props.accent}>
      <Group class="mt-[8px]">
        <div class="focal-bevel px-pad pt-[16px] pb-[20px]">
          <div class="flex items-baseline justify-between">
            <span class="text-card font-bold">X70_001.IMG</span>
            <span class="text-detail text-secondary">/BACKUP</span>
          </div>
          <p class="mt-[10px] text-detail text-secondary">{BACKUP_NOTE}</p>
          <div class="mt-[16px] flex justify-end text-detail text-secondary">
            <Show when={props.fraction < 1} fallback={<Check size={22} class="text-success" />}>
              {Math.round(props.fraction * total)} / {total} MiB
            </Show>
          </div>
          <div class="mt-[10px]">
            <ProgressBar fraction={props.fraction} />
          </div>
        </div>
      </Group>
    </Screen>
  );
}
