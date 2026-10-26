import { render } from '@solidjs/web';
import { createSignal, flush } from 'solid-js';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import { CardBrowser } from './card';
import type { Engine } from '../engine/api';
import type { CardEntry, CardState } from 'fujimod/emulator';
import { flags } from '../flags';
import { downloadBlob } from './download';
import { zipBlob } from '../archive';
vi.mock('./download', () => ({ downloadBlob: vi.fn() }));
vi.mock('../archive', () => ({ zipBlob: vi.fn(async () => new Blob()) }));
vi.mock('../flags', () => ({
  flags: { allowFpUpdateDownload: false, allowSdCardBootDownload: false },
}));
let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
  flags.allowFpUpdateDownload = false;
  flags.allowSdCardBootDownload = false;
  vi.clearAllMocks();
});
const state: CardState = {
  supported: true,
  present: true,
  protected: false,
  busy: false,
  revision: 1,
  capacity_bytes: 1024 ** 3,
  data_faults: [],
  fault_history: [],
};
test('card observations follow engine replacement, busy/insertion state and fresh revisions', async () => {
  let publish!: (state: CardState) => void;
  const stop = vi.fn(),
    cardTree = vi.fn(async () => ({
      revision: 1,
      entries: [
        {
          kind: 'directory',
          name: 'DCIM',
          path: '/DCIM',
          bytes: 0,
          children: [{ kind: 'file', name: 'photo.JPG', path: '/DCIM/photo.JPG', bytes: 2048 }],
        },
      ],
    }));
  const engine = {
    cardChanges: (listener: typeof publish) => {
      publish = listener;
      return stop;
    },
    cardTree,
  } as unknown as Engine;
  const [active, setActive] = createSignal<Engine>();
  const present = vi.fn(async () => {}),
    format = vi.fn(async () => {});
  dispose = render(
    () => <CardBrowser engine={active()} present={present} format={format} />,
    document.body,
  );
  flush();
  setActive(engine);
  flush();
  publish(state);
  await Promise.resolve();
  flush();
  expect(document.querySelector('#card-capacity')?.textContent).toBe('FAT32 · 1 GB');
  expect(document.querySelector('#card-files')?.textContent).toContain('photo.JPG');
  const folder = document.querySelector('.folder button') as HTMLButtonElement;
  folder.click();
  flush();
  expect(folder.getAttribute('aria-expanded')).toBe('false');
  folder.click();
  flush();
  expect(folder.getAttribute('aria-expanded')).toBe('true');
  publish({ ...state, busy: true });
  flush();
  expect(
    (document.querySelector('[data-card-present="false"]') as HTMLButtonElement).disabled,
  ).toBe(true);
  publish({ ...state, busy: false });
  flush();
  for (const value of [false, true]) {
    (document.querySelector(`[data-card-present="${value}"]`) as HTMLButtonElement).click();
    expect(present).toHaveBeenLastCalledWith(value);
    publish({ ...state, present: value });
    flush();
    expect(
      document.querySelector(`[data-card-present="${value}"]`)?.getAttribute('aria-pressed'),
    ).toBe('true');
  }
  setActive(undefined);
  flush();
  expect(stop).toHaveBeenCalledOnce();
  expect(document.querySelector('#card-files')?.textContent).toBe('');
  expect((document.querySelector('#card-format') as HTMLButtonElement).disabled).toBe(true);
});
test('late reads cannot repopulate an ejected card', async () => {
  let publish!: (state: CardState) => void, finish!: (value: unknown) => void;
  const engine = {
    cardChanges: (listener: typeof publish) => {
      publish = listener;
      return () => {};
    },
    cardTree: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  } as unknown as Engine;
  dispose = render(
    () => (
      <CardBrowser
        engine={engine}
        present={() => Promise.resolve()}
        format={() => Promise.resolve()}
      />
    ),
    document.body,
  );
  flush();
  publish(state);
  publish({ ...state, present: false });
  finish({ revision: 1, entries: [{ kind: 'file', path: '/stale', name: 'stale', bytes: 1 }] });
  await Promise.resolve();
  flush();
  expect(document.querySelector('#card-files')?.textContent).toBe('');
});

test('fresh SD listings preserve unchanged rows and collapsed folders across revisions', async () => {
  let publish!: (state: CardState) => void;
  const engine = {
    cardChanges: (listener: typeof publish) => {
      publish = listener;
      return () => {};
    },
    cardTree: async (revision: number) => ({
      revision,
      entries: [
        {
          kind: 'directory',
          name: 'DCIM',
          path: '/DCIM',
          bytes: 0,
          children: (revision === 1
            ? ['a.JPG', 'b.JPG', 'c.JPG']
            : ['a.JPG', 'b.JPG', 'c.JPG', 'd.JPG']
          ).map((name) => ({ kind: 'file', name, path: `/DCIM/${name}`, bytes: 1024 })),
        },
      ],
    }),
  } as unknown as Engine;
  dispose = render(
    () => (
      <CardBrowser
        engine={engine}
        present={() => Promise.resolve()}
        format={() => Promise.resolve()}
      />
    ),
    document.body,
  );
  flush();
  publish(state);
  await Promise.resolve();
  flush();
  const folder = document.querySelector<HTMLButtonElement>('.folder button')!;
  const photo = document.querySelector('button[aria-label="Download a.JPG"]');
  folder.click();
  flush();
  publish({ ...state, revision: 2 });
  await Promise.resolve();
  flush();
  expect(document.querySelector('.folder button')).toBe(folder);
  expect(folder.getAttribute('aria-expanded')).toBe('false');
  expect(document.querySelector('button[aria-label="Download a.JPG"]')).toBe(photo);
  expect(document.querySelector('#card-files')?.textContent).toContain('d.JPG');
});

test('repeated observations of the same revision retain an in-flight listing', async () => {
  let publish!: (state: CardState) => void, finish!: (value: unknown) => void;
  const listing = vi.fn(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const engine = {
    cardChanges: (listener: typeof publish) => {
      publish = listener;
      return () => {};
    },
    cardTree: listing,
  } as unknown as Engine;
  dispose = render(
    () => (
      <CardBrowser
        engine={engine}
        present={() => Promise.resolve()}
        format={() => Promise.resolve()}
      />
    ),
    document.body,
  );
  flush();
  publish(state);
  publish({ ...state });
  publish({ ...state });
  expect(listing).toHaveBeenCalledOnce();
  finish({
    revision: 1,
    entries: [{ kind: 'file', name: 'photo.JPG', path: '/photo.JPG', bytes: 1 }],
  });
  await Promise.resolve();
  flush();
  expect(document.querySelector('#card-files')?.textContent).toContain('photo.JPG');
});

test.each([
  { allowFpUpdateDownload: false, allowSdCardBootDownload: false },
  { allowFpUpdateDownload: true, allowSdCardBootDownload: false },
  { allowFpUpdateDownload: false, allowSdCardBootDownload: true },
  { allowFpUpdateDownload: true, allowSdCardBootDownload: true },
])('card firmware downloads follow the SD release gate: %j', async (gates) => {
  Object.assign(flags, gates);
  let publish!: (state: CardState) => void;
  const bootFiles: CardEntry[] = [
    {
      kind: 'file',
      name: 'FFBoot.BIN',
      path: '/FFBoot.BIN',
      bytes: gates.allowSdCardBootDownload ? 2 : 64 * 1024 ** 2,
      children: [],
    },
    {
      kind: 'file',
      name: 'ffsys.bin',
      path: '/ffsys.bin',
      bytes: gates.allowSdCardBootDownload ? 2 : 64 * 1024 ** 2,
      children: [],
    },
  ];
  const nestedBoot: CardEntry = {
    kind: 'file',
    name: 'FFSYS.BIN',
    path: '/DCIM/FFSYS.BIN',
    bytes: 2,
    children: [],
  };
  const photo: CardEntry = {
    kind: 'file',
    name: 'photo.JPG',
    path: '/DCIM/photo.JPG',
    bytes: 2,
    children: [],
  };
  const backup: CardEntry = {
    kind: 'file',
    name: 'ffboot.bin.bak',
    path: '/ffboot.bin.bak',
    bytes: 2,
    children: [],
  };
  const readCardFile = vi.fn<Engine['readCardFile']>(async () => new Uint8Array([1, 2]));
  const engine = {
    cardChanges: (listener: typeof publish) => {
      publish = listener;
      return () => {};
    },
    cardTree: async () => ({
      revision: 1,
      entries: [
        ...bootFiles,
        { kind: 'directory', name: 'DCIM', path: '/DCIM', bytes: 0, children: [nestedBoot, photo] },
        backup,
      ],
    }),
    readCardFile,
  } as unknown as Engine;
  dispose = render(
    () => (
      <CardBrowser
        engine={engine}
        present={() => Promise.resolve()}
        format={() => Promise.resolve()}
      />
    ),
    document.body,
  );
  flush();
  publish(state);
  await Promise.resolve();
  flush();
  const allowed = gates.allowSdCardBootDownload
    ? [...bootFiles, nestedBoot, photo, backup]
    : [photo, backup];
  expect(
    [...document.querySelectorAll('#card-files .dl')].map((button) =>
      button.getAttribute('aria-label'),
    ),
  ).toEqual(allowed.map((entry) => `Download ${entry.name}`));
  for (const entry of allowed) {
    document
      .querySelector<HTMLButtonElement>(`button[aria-label="Download ${entry.name}"]`)!
      .click();
    await vi.waitFor(() =>
      expect(downloadBlob).toHaveBeenLastCalledWith(expect.any(Blob), entry.name),
    );
    expect(readCardFile).toHaveBeenLastCalledWith(entry.path, { revision: 1 });
  }
  readCardFile.mockClear();
  document.querySelector<HTMLButtonElement>('#card-download')!.click();
  await vi.waitFor(() =>
    expect(downloadBlob).toHaveBeenLastCalledWith(expect.any(Blob), 'sd-card.zip'),
  );
  expect(readCardFile.mock.calls.map((call) => call[0])).toEqual(
    allowed.map((entry) => entry.path),
  );
  expect(zipBlob).toHaveBeenLastCalledWith(
    allowed.map((entry) => ({ path: entry.path.slice(1), data: expect.any(Blob) })),
  );
});

test('a card containing only gated firmware files has no downloads', async () => {
  let publish!: (state: CardState) => void;
  const readCardFile = vi.fn();
  const engine = {
    cardChanges: (listener: typeof publish) => {
      publish = listener;
      return () => {};
    },
    cardTree: async () => ({
      revision: 1,
      entries: ['ffboot.bin', 'ffsys.bin'].map((name) => ({
        kind: 'file',
        name,
        path: `/${name}`,
        bytes: 64 * 1024 ** 2,
        children: [],
      })),
    }),
    readCardFile,
  } as unknown as Engine;
  dispose = render(
    () => (
      <CardBrowser
        engine={engine}
        present={() => Promise.resolve()}
        format={() => Promise.resolve()}
      />
    ),
    document.body,
  );
  flush();
  publish(state);
  await Promise.resolve();
  flush();
  expect(document.querySelector('#card-files')?.textContent).toBe('');
  const all = document.querySelector<HTMLButtonElement>('#card-download')!;
  expect(all.disabled).toBe(true);
  all.click();
  expect(readCardFile).not.toHaveBeenCalled();
  expect(downloadBlob).not.toHaveBeenCalled();
});
