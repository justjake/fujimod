import { render } from '@solidjs/web';
import { For, Match, Switch, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { HUE_STEP, HUE_STEPS, SWATCHES } from './components';
import {
  BackupPage,
  BootPage,
  INITIAL_SETUP,
  OLDER,
  RELEASE_FILES,
  SetupPage,
  SHUT_DOWN_STYLES,
  START_UP_STYLES,
  SWATCH_INDEX,
  UpdatePage,
  setupTargets,
  type BootState,
  type SetupState,
  type SetupTarget,
  type UpdateState,
} from './fujimod-setup';
import './focal.css';

/** Shows a 720x480 screen at camera size, scaled down to fit narrow viewports. */
function Fit(props: { children: JSX.Element }) {
  let host: HTMLDivElement | undefined;
  const [scale, setScale] = createSignal(1);
  onSettled(() => {
    const element = host;
    if (!element) return;
    const observer = new ResizeObserver(() => setScale(Math.min(1, element.clientWidth / 720)));
    observer.observe(element);
    return () => observer.disconnect();
  });
  return (
    <div
      ref={(element) => (host = element)}
      class="w-full max-w-[720px]"
      style={{ height: `${480 * scale()}px` }}
    >
      <div class="origin-top-left" style={{ transform: `scale(${scale()})`, width: '720px' }}>
        {props.children}
      </div>
    </div>
  );
}

/* ---- The live device ------------------------------------------------------------ */

type Page = 'setup' | 'update' | 'boot' | 'backup';

function Device() {
  const [page, setPage] = createSignal<Page>('setup');
  const [setup, setSetup] = createSignal<SetupState>(INITIAL_SETUP);
  const [update, setUpdate] = createSignal<UpdateState>({
    phase: 'checking',
    slotB: null,
    files: RELEASE_FILES,
    accent: INITIAL_SETUP.colors.setup,
    hue: null,
  });
  const [boot, setBoot] = createSignal<BootState>({
    focus: 'MANUAL',
    boot: 'MANUAL',
    backedUp: false,
    accent: INITIAL_SETUP.colors.setup,
    hue: null,
  });
  const [backup, setBackup] = createSignal(0);
  let timer: ReturnType<typeof setInterval> | undefined;
  const stop = () => clearInterval(timer);
  const patch = (change: Partial<SetupState>) => setSetup({ ...setup(), ...change });
  const theme = () => ({ accent: setup().colors.setup, hue: setup().hue });

  const openUpdate = () => {
    setUpdate({ phase: 'checking', slotB: null, files: RELEASE_FILES, ...theme() });
    setPage('update');
    stop();
    timer = setTimeout(() => setUpdate({ ...update(), phase: 'available' }), 1200);
  };
  const download = () => {
    setUpdate({ ...update(), phase: 'downloading' });
    stop();
    timer = setInterval(() => {
      const files = update().files.map((f) => ({ ...f }));
      const next = files.find((f) => f.state !== 'present' && (f.state as number) < 1);
      if (!next) {
        stop();
        setUpdate({ ...update(), phase: 'downloaded' });
        return;
      }
      next.state = Math.min(1, (next.state as number) + 0.12 / next.size);
      setUpdate({ ...update(), files });
    }, 60);
  };
  const openBackup = () => {
    setBackup(0);
    setPage('backup');
    stop();
    timer = setInterval(() => {
      const fraction = Math.min(1, backup() + 0.02);
      setBackup(fraction);
      if (fraction >= 1) {
        stop();
        setBoot({ ...boot(), backedUp: true });
      }
    }, 50);
  };
  const leave = () => {
    stop();
    if (page() === 'backup' && boot().focus === 'backup' && setup().focus !== 'backup')
      setPage('boot');
    else {
      if (page() === 'update') patch({ lastChecked: 'today 9:41 AM' });
      if (page() === 'boot') patch({ boot: boot().boot });
      setPage('setup');
    }
  };

  const select = (target: SetupTarget) => {
    const s = setup();
    switch (target) {
      case 'wifi':
        return patch({ connected: !s.connected });
      case 'updates':
        return s.connected && openUpdate();
      case 'change':
        setBoot({ ...boot(), focus: 'MANUAL', boot: s.boot, ...theme() });
        return setPage('boot');
      case 'backup':
        return openBackup();
      case 'rec':
      case 'setup':
      case 'play':
        return patch({
          popup: {
            kind: 'swatch',
            row: target,
            cursor: Math.max(0, SWATCH_INDEX(s.colors[target])),
          },
        });
      case 'background':
        return patch({ popup: { kind: 'hue', original: s.hue } });
      case 'startUp':
        return patch({ popup: { kind: 'choice', row: 'startUp', cursor: s.startUp } });
      case 'shutDown':
        return patch({ popup: { kind: 'choice', row: 'shutDown', cursor: s.shutDown } });
      default:
        return;
    }
  };

  const setupKey = (key: string) => {
    const s = setup();
    const popup = s.popup;
    if (popup?.kind === 'swatch') {
      const moves: Record<string, number> = {
        ArrowLeft: -1,
        ArrowRight: 1,
        ArrowUp: -4,
        ArrowDown: 4,
      };
      if (key in moves) {
        const cursor = (popup.cursor + moves[key] + SWATCHES.length) % SWATCHES.length;
        return patch({ popup: { ...popup, cursor } });
      }
      if (key === 'Enter')
        return patch({
          colors: { ...s.colors, [popup.row]: SWATCHES[popup.cursor] },
          popup: undefined,
        });
      if (key === 'Escape') return patch({ popup: undefined });
      return;
    }
    if (popup?.kind === 'hue') {
      if (key === 'ArrowLeft' || key === 'ArrowRight') {
        const index = s.hue === null ? -1 : s.hue / HUE_STEP;
        const next = Math.max(-1, Math.min(HUE_STEPS - 1, index + (key === 'ArrowLeft' ? -1 : 1)));
        return patch({ hue: next < 0 ? null : next * HUE_STEP });
      }
      if (key === 'Enter') return patch({ popup: undefined });
      if (key === 'Escape') return patch({ hue: popup.original, popup: undefined });
      return;
    }
    if (popup?.kind === 'choice') {
      const count = (popup.row === 'startUp' ? START_UP_STYLES : SHUT_DOWN_STYLES).length;
      if (key === 'ArrowUp' || key === 'ArrowDown') {
        const cursor = (popup.cursor + (key === 'ArrowUp' ? -1 : 1) + count) % count;
        return patch({ popup: { ...popup, cursor } });
      }
      if (key === 'Enter') return patch({ [popup.row]: popup.cursor, popup: undefined });
      if (key === 'Escape') return patch({ popup: undefined });
      return;
    }
    const targets = setupTargets(s);
    const index = targets.indexOf(s.focus);
    if (key === 'ArrowUp' || key === 'ArrowDown') {
      const next = Math.max(0, Math.min(targets.length - 1, index + (key === 'ArrowUp' ? -1 : 1)));
      return patch({ focus: targets[next] });
    }
    if (key === 'Enter') return select(s.focus);
  };

  const bootKey = (key: string) => {
    const b = boot();
    const order: BootState['focus'][] = ['MANUAL', 'AUTOMATIC', 'backup'];
    if (key === 'ArrowUp' || key === 'ArrowDown') {
      const index = order.indexOf(b.focus) + (key === 'ArrowUp' ? -1 : 1);
      return setBoot({ ...b, focus: order[Math.max(0, Math.min(order.length - 1, index))] });
    }
    if (key === 'Enter') {
      if (b.focus === 'backup') return openBackup();
      if (b.focus === 'MANUAL' || b.backedUp) return setBoot({ ...b, boot: b.focus });
    }
    if (key === 'Escape') leave();
  };

  const onKey = (event: KeyboardEvent) => {
    const key = event.key === 'Backspace' ? 'Escape' : event.key;
    if (!['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Enter', 'Escape'].includes(key))
      return;
    event.preventDefault();
    if (page() === 'setup') {
      if (key === 'Escape' && !setup().popup) return;
      return setupKey(key);
    }
    if (page() === 'boot') return bootKey(key);
    if (page() === 'update') {
      if (key === 'Enter' && update().phase === 'available') return download();
      if (key === 'Enter' && update().phase === 'downloaded') return leave();
      if (key === 'Escape') return leave();
      return;
    }
    if (key === 'Escape') leave();
  };

  onSettled(() => {
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('keydown', onKey);
      stop();
    };
  });

  return (
    <Fit>
      <Switch>
        <Match when={page() === 'setup'}>
          <SetupPage
            state={setup()}
            onSelect={(target) => {
              patch({ focus: target, popup: undefined });
              select(target);
            }}
          />
        </Match>
        <Match when={page() === 'update'}>
          <UpdatePage state={update()} />
        </Match>
        <Match when={page() === 'boot'}>
          <BootPage state={boot()} />
        </Match>
        <Match when={page() === 'backup'}>
          <BackupPage fraction={backup()} accent={setup().colors.setup} hue={setup().hue} />
        </Match>
      </Switch>
    </Fit>
  );
}

/* ---- Gallery ---------------------------------------------------------------------- */

const accent = INITIAL_SETUP.colors.setup;
const update = (change: Partial<UpdateState>): UpdateState => ({
  phase: 'available',
  slotB: null,
  files: RELEASE_FILES,
  accent,
  hue: null,
  ...change,
});
const downloading = RELEASE_FILES.map((file) =>
  file.state === 'present'
    ? file
    : {
        ...file,
        state: file.name === 'fujimod.bin' ? 1 : file.name === 'firmware.patch' ? 0.5 : 0,
      },
);
const setupState = (change: Partial<SetupState>): SetupState => ({ ...INITIAL_SETUP, ...change });

export const STATES: { id: string; title: string; note?: string; view: () => JSX.Element }[] = [
  {
    id: 'setup-top',
    title: 'FUJIMOD SET-UP',
    note: 'Check for updates stays disabled until Wi-Fi connects.',
    view: () => <SetupPage state={setupState({})} />,
  },
  {
    id: 'setup-connected',
    title: 'Connected',
    view: () => (
      <SetupPage
        state={setupState({ connected: true, lastChecked: 'today 9:41 AM', focus: 'updates' })}
      />
    ),
  },
  {
    id: 'setup-change',
    title: 'Boot Setup > CHANGE',
    view: () => <SetupPage state={setupState({ focus: 'change' })} />,
  },
  {
    id: 'setup-appearance',
    title: 'Appearance',
    note: 'The focus stripe on this page uses SET-UP COLOR.',
    view: () => <SetupPage state={setupState({ focus: 'rec' })} />,
  },
  {
    id: 'setup-background',
    title: 'BACKGROUND COLOR, gray',
    view: () => <SetupPage state={setupState({ focus: 'background' })} />,
  },
  {
    id: 'hue-picker',
    title: 'BACKGROUND COLOR picker',
    note: 'LEFT/RIGHT step through the default gray and 24 hues; the page retints as the selection moves, every gray taking the hue at its own value.',
    view: () => (
      <SetupPage
        state={setupState({
          focus: 'background',
          hue: 210,
          popup: { kind: 'hue', original: null },
        })}
      />
    ),
  },
  {
    id: 'hue-reset',
    title: 'BACKGROUND COLOR picker, default gray',
    view: () => (
      <SetupPage
        state={setupState({
          focus: 'background',
          hue: null,
          popup: { kind: 'hue', original: 210 },
        })}
      />
    ),
  },
  {
    id: 'setup-background-tinted',
    title: 'BACKGROUND COLOR, blue',
    view: () => <SetupPage state={setupState({ focus: 'background', hue: 210 })} />,
  },
  {
    id: 'setup-orange',
    title: 'SET-UP COLOR orange, Automatic boot',
    view: () => (
      <SetupPage
        top
        state={setupState({
          focus: 'updates',
          connected: true,
          lastChecked: 'today 9:41 AM',
          boot: 'AUTOMATIC',
          colors: { ...INITIAL_SETUP.colors, setup: '#e85c00' },
        })}
      />
    ),
  },
  {
    id: 'setup-bottom',
    title: 'Bottom',
    view: () => <SetupPage state={setupState({ focus: 'shutDown' })} />,
  },
  {
    id: 'color-picker',
    title: 'REC COLOR picker',
    note: 'The check marks the current color, the ring the cursor.',
    view: () => (
      <SetupPage
        state={setupState({ focus: 'rec', popup: { kind: 'swatch', row: 'rec', cursor: 1 } })}
      />
    ),
  },
  {
    id: 'start-up-style',
    title: 'START UP STYLE',
    view: () => (
      <SetupPage
        state={setupState({
          focus: 'startUp',
          popup: { kind: 'choice', row: 'startUp', cursor: 0 },
        })}
      />
    ),
  },
  {
    id: 'checking',
    title: 'Checking',
    view: () => <UpdatePage state={update({ phase: 'checking' })} />,
  },
  {
    id: 'up-to-date',
    title: 'Up to date',
    view: () => <UpdatePage state={update({ phase: 'up-to-date' })} />,
  },
  {
    id: 'available-top',
    title: 'Update available',
    note: 'DOWN scrolls to DOWNLOAD.',
    view: () => <UpdatePage top state={update({})} />,
  },
  {
    id: 'available',
    title: 'Update available: add B',
    view: () => <UpdatePage state={update({})} />,
  },
  {
    id: 'available-replace',
    title: 'Update available: replace B',
    view: () => <UpdatePage state={update({ slotB: OLDER })} />,
  },
  {
    id: 'downloading',
    title: 'Downloading',
    note: 'One segment per file: fujimod.bin done, firmware.patch half, cert.pem already on the SD card (gray), zones.json waiting.',
    view: () => <UpdatePage state={update({ phase: 'downloading', files: downloading })} />,
  },
  {
    id: 'downloaded',
    title: 'Downloaded',
    view: () => <UpdatePage state={update({ phase: 'downloaded' })} />,
  },
  {
    id: 'boot-setup',
    title: 'BOOT SETUP before a backup',
    view: () => (
      <BootPage state={{ focus: 'MANUAL', boot: 'MANUAL', backedUp: false, accent, hue: null }} />
    ),
  },
  {
    id: 'boot-setup-backed-up',
    title: 'BOOT SETUP after a backup',
    view: () => (
      <BootPage state={{ focus: 'AUTOMATIC', boot: 'MANUAL', backedUp: true, accent, hue: null }} />
    ),
  },
  {
    id: 'backing-up',
    title: 'Backing up',
    view: () => <BackupPage fraction={0.375} accent={accent} hue={null} />,
  },
];

function Review() {
  return (
    <div class="min-h-dvh bg-[#111] p-[16px] font-sans text-[15px] text-[#ddd]">
      <h1 class="text-[22px] font-semibold">FUJIMOD SET-UP</h1>
      <section class="mt-[16px]">
        <h2 class="text-[17px] font-semibold">Live</h2>
        <p class="mt-[4px] mb-[10px] text-[#999]">
          Arrow keys are the selector, Enter is MENU/OK, Esc is DISP/BACK. Tap a row to select it.
        </p>
        <Device />
      </section>
      <For each={STATES}>
        {(state) => (
          <section id={state.id} class="mt-[32px]">
            <h2 class="text-[17px] font-semibold">{state.title}</h2>
            <p class="mt-[4px] mb-[10px] min-h-[1em] text-[#999]">{state.note}</p>
            <Fit>{state.view()}</Fit>
          </section>
        )}
      </For>
    </div>
  );
}

const host = document.getElementById('focal-review');
if (!host) throw new Error('Missing #focal-review');
if (import.meta.env.DEV) render(() => <Review />, host);
