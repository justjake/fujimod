import { For, Show, createEffect, createMemo, createSignal } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import { formatBytes } from '@fujimod/emulator-ui/ui/file-list';
import { downloadBlob } from '@fujimod/emulator-ui/ui/download';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import type { Firmware } from '../../catalog';
import type { SdCardFile } from '../../patch/catalog';
import type { LoadedRelease } from '../../patch/release';
import { loadOtaRelease, storeName, type SdBootV2Release } from '../../ota/release';
import {
  sdBootV2Files,
  sdBootV2For,
  sdBootV2Plan,
  conflicting,
  plannedBytes,
  releaseCameras,
  type CardCamera,
} from './plan';
import { buildCard } from './build';
import { SdBootV2Shape } from './SdBootV2Shape';
import './sd-card.css';

/** The recipes list holds 255 files; seven are pinned to C1–C7 in the Q menu. */
const recipeCapacity = 255;

function Check(props: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  onChange: (checked: boolean) => void;
}) {
  return (
    <input
      type="checkbox"
      class="sd-check"
      aria-label={props.label}
      checked={props.checked}
      disabled={props.disabled}
      onChange={(event) => props.onChange(event.currentTarget.checked)}
    />
  );
}

/** Documentation for the rows below it, like a comment in the card's listing. */
function Note(props: { title?: string; children: JSX.Element }) {
  return (
    <div class="sd-note">
      <Show when={props.title}>
        <b>{props.title}</b>
      </Show>
      {props.children}
    </div>
  );
}

function FileRow(props: {
  name: string;
  detail?: string;
  bytes?: number;
  dim?: boolean;
  folder?: boolean;
  check?: JSX.Element;
  after?: JSX.Element;
}) {
  return (
    <div class={{ r: true, 'sd-row': true, dim: props.dim, folder: props.folder }}>
      <span class="sd-slot">{props.check}</span>
      <span class="tw" />
      <Icon name={props.folder ? 'folder' : 'file'} />
      <span class="n">
        {props.name}
        <Show when={props.detail}>
          <span class="sd-detail">{props.detail}</span>
        </Show>
      </span>
      {props.after}
      <Show when={props.bytes !== undefined}>
        <span class="s">{formatBytes(props.bytes!)}</span>
      </Show>
    </div>
  );
}

function Folder(props: {
  name: string;
  dim?: boolean;
  check?: JSX.Element;
  children: JSX.Element;
}) {
  const [open, setOpen] = createSignal(true);
  return (
    <>
      <div class={{ r: true, 'sd-row': true, folder: true, dim: props.dim }}>
        <span class="sd-slot">{props.check}</span>
        <button
          type="button"
          class="fold"
          aria-expanded={open() ? 'true' : 'false'}
          aria-label={props.name}
          onClick={() => setOpen((value) => !value)}
        >
          <span class={['tw', { closed: !open() }]}>
            <Icon name="chevron" size={13} />
          </span>
          <Icon name="folder" />
          <span class="n">{props.name}</span>
        </button>
      </div>
      <div class="kids" hidden={!open()}>
        {props.children}
      </div>
    </>
  );
}

/** Optional card files in one folder, each with its own include box and one for the folder. */
function OptionalFolder(props: {
  name: string;
  files: readonly SdCardFile[];
  excluded: ReadonlySet<string>;
  setExcluded: (paths: readonly string[], excluded: boolean) => void;
  dim: boolean;
}) {
  const included = () => props.files.filter((file) => !props.excluded.has(file.path)).length;
  return (
    <Folder
      name={props.name}
      dim={props.dim}
      check={
        <Show when={props.files.length}>
          <input
            type="checkbox"
            class="sd-check"
            aria-label={`Include every file in ${props.name}`}
            checked={included() === props.files.length}
            ref={(element) =>
              createEffect(
                () => included() > 0 && included() < props.files.length,
                (mixed) => {
                  element.indeterminate = mixed;
                },
              )
            }
            onChange={(event) =>
              props.setExcluded(
                props.files.map((file) => file.path),
                !event.currentTarget.checked,
              )
            }
          />
        </Show>
      }
    >
      <For each={props.files} keyed={(file) => file.path}>
        {(file) => (
          <FileRow
            name={file().path.split('/').at(-1)!}
            detail={[file().title, file().author].filter(Boolean).join(' · ')}
            bytes={file().bytes}
            dim={props.dim || props.excluded.has(file().path)}
            check={
              <Check
                label={`Include ${file().path.split('/').at(-1)}`}
                checked={!props.excluded.has(file().path)}
                onChange={(checked) => props.setExcluded([file().path], !checked)}
              />
            }
          />
        )}
      </For>
    </Folder>
  );
}

export function SdBootV2Download(props: { release?: LoadedRelease; catalog: readonly Firmware[] }) {
  const card = () => props.release?.sd_card_files ?? [];
  const cameras = createMemo(() =>
    props.release ? releaseCameras(props.release.firmwares, props.catalog, card()) : [],
  );
  const [chosenIds, setChosenIds] = createSignal<ReadonlySet<string>>(new Set());
  const [excluded, setExcludedPaths] = createSignal<ReadonlySet<string>>(new Set());
  const [git, setGit] = createSignal(false);
  const [releases, setReleases] = createSignal<ReadonlyMap<string, SdBootV2Release | Error>>(new Map());
  const [busy, setBusy] = createSignal(false);
  const [progress, setProgress] = createSignal('');
  const [error, setError] = createSignal('');

  const chosen = createMemo(() => cameras().filter((camera) => chosenIds().has(camera.id)));
  const blocked = createMemo(() => conflicting(cameras(), chosen(), card()));
  /** The rows document the chosen cameras, or preview the first camera, dimmed. */
  const shown = createMemo(() => (chosen().length ? chosen() : cameras().slice(0, 1)));
  const previewing = () => !chosen().length;
  const processors = createMemo(() => [...new Set(shown().map((camera) => camera.processor))]);
  const optional = createMemo(() => card().filter((file) => file.processor === undefined));
  const inFolder = (folder: string) =>
    optional().filter((file) => file.path.startsWith(`FUJIMOD/${folder}/`));
  const loaded = createMemo(() => {
    const map = new Map<string, SdBootV2Release>();
    for (const [id, value] of releases()) if (!(value instanceof Error)) map.set(id, value);
    return map;
  });
  const plan = createMemo(() =>
    sdBootV2Plan({ cameras: chosen(), card: card(), releases: loaded(), excluded: excluded() }),
  );
  const pending = () => chosen().some((camera) => !releases().has(camera.id));
  const store = createMemo(() => {
    const files = new Map<string, { bytes: number; paths: string[] }>();
    for (const camera of chosen()) {
      const release = loaded().get(camera.id);
      for (const [path, file] of Object.entries(release?.fs ?? {})) {
        const name = storeName(file.sha256);
        const entry = files.get(name) ?? { bytes: file.size, paths: [] };
        if (!entry.paths.includes(path)) entry.paths.push(path);
        files.set(name, entry);
      }
    }
    return [...files].sort(([a], [b]) => a.localeCompare(b));
  });

  function choose(camera: CardCamera, checked: boolean) {
    setChosenIds((ids) => {
      const next = new Set(ids);
      if (checked) next.add(camera.id);
      else next.delete(camera.id);
      return next;
    });
    setError('');
    const base = props.release?.assetBase;
    if (!checked || releases().has(camera.id) || !base) return;
    void loadOtaRelease(base, camera.id)
      .catch((reason: unknown) => Error(shownError(reason)))
      .then((value) => setReleases((map) => new Map(map).set(camera.id, value)));
  }
  function setExcluded(paths: readonly string[], value: boolean) {
    setExcludedPaths((set) => {
      const next = new Set(set);
      for (const path of paths) {
        if (value) next.add(path);
        else next.delete(path);
      }
      return next;
    });
  }
  async function download() {
    const release = props.release;
    if (!release || plan().problems.length || busy()) return;
    setBusy(true);
    setError('');
    try {
      const zip = await buildCard(plan().files, {
        assetBase: release.assetBase,
        base: location.origin,
        release: release.release,
        git: git(),
        progress: (done, total) => setProgress(`${done} of ${total} files`),
      });
      downloadBlob(zip, `fujimod-sd-card-${release.release}.zip`);
    } catch (reason) {
      setError(shownError(reason));
    } finally {
      setBusy(false);
      setProgress('');
    }
  }

  const problem = () => {
    if (!chosen().length || pending()) return '';
    const failed = chosen()
      .map((camera) => releases().get(camera.id))
      .find((value): value is Error => value instanceof Error);
    return failed?.message ?? plan().problems[0] ?? '';
  };

  return (
    <Show when={cameras().length}>
      <section class="sd-download" aria-labelledby="download">
        <h2 class="section-heading" id="download">
          Download
        </h2>
        <div class="sd-cameras" role="group" aria-label="Cameras">
          <For each={cameras()} keyed={(camera) => camera.id}>
            {(camera) => (
              <label
                class={{ 'sd-camera': true, blocked: blocked().has(camera().id) }}
                title={
                  blocked().has(camera().id)
                    ? `${camera().processor} SD boot files differ from the chosen cameras’.`
                    : undefined
                }
              >
                <input
                  type="checkbox"
                  class="sd-check"
                  checked={chosenIds().has(camera().id)}
                  disabled={blocked().has(camera().id)}
                  onChange={(event) => choose(camera(), event.currentTarget.checked)}
                />
                <span class="sd-camera-name">
                  <b>{camera().model}</b>
                  <span>{camera().processor}</span>
                </span>
                <span class="sd-camera-version">{camera().version}</span>
                <span class="sd-camera-mods">
                  {camera().mods} {camera().mods === 1 ? 'mod' : 'mods'}
                </span>
              </label>
            )}
          </For>
        </div>
        <div class="sd-card">
          <SdBootV2Shape />
          <div class="sd-label">
            <img src="/brand.png" alt="FujiMod" width="110" height="28" />
            <span>FAT32</span>
          </div>
          <div class="tree sd-tree" id="sd-card-files">
            <For each={processors()}>
              {(processor) => {
                const boot = () => sdBootV2For(processor);
                const firmware = () =>
                  shown().find((camera) => camera.processor === processor)?.id ?? '';
                const files = () => sdBootV2Files(card(), processor, firmware());
                const size = (name: string) =>
                  files().files.find((file) => file.path === name)?.bytes;
                return (
                  <Show when={boot()}>
                    {(boot) => (
                      <>
                        <Note title="SD boot v2 loader (stage 1)">
                          <p>
                            Your camera’s built-in boot ROM is the first code that runs when you
                            turn your camera on. Hold the special key combination only while turning
                            the power switch on. Once the switch is in the ON position, release the
                            keys; the boot ROM will execute this file.
                          </p>
                          <p>
                            This file carries the chosen camera’s own early boot indicator setup
                            and is written only for it, so one SD card serves one camera body.
                          </p>
                          <p>
                            All it does is execute the appropriate FM_*.BIN file for the current
                            camera.
                          </p>
                        </Note>
                        <FileRow name="FFBOOT.BIN" bytes={size('FFBOOT.BIN')} dim={previewing()} />
                        <Note>
                          <p>
                            The boot ROM reads this file before FFBOOT.BIN and applies its clock,
                            memory and register settings.
                          </p>
                        </Note>
                        <FileRow name="FFSYS.BIN" bytes={size('FFSYS.BIN')} dim={previewing()} />
                        <Note title="FujiMod bootloader (Stage 2)">
                          <p>
                            If you like FujiMod and trust it enough to modify your camera’s internal
                            firmware, it can modify your camera’s firmware to automatically check
                            for this file during boot and execute it if it exists. If the file
                            doesn’t exist, your camera will boot into the original Fujifilm
                            firmware. This is the smallest change we can make to your camera’s
                            internal storage to enable automatic boot of FujiMod.
                          </p>
                          <p>
                            This file is the same for all cameras with the {boot().name} processor.
                            We use a different name per processor so that you can share a single
                            FujiMod SD card between any supported cameras.
                          </p>
                          <p>
                            This file executes the FujiMod firmware patcher specific to the current
                            camera’s original firmware, named by /FUJIMOD/SYSTEM/{firmware()}/A.JSN
                            or B.JSN.
                          </p>
                        </Note>
                        <FileRow
                          name={boot().stage2}
                          bytes={size(boot().stage2)}
                          dim={previewing()}
                        />
                      </>
                    )}
                  </Show>
                );
              }}
            </For>
            <Note>
              <p>FujiMod lives on your SD card. Eject or format the card to uninstall.</p>
            </Note>
            <Folder name="FUJIMOD">
              <Note>
                <p>
                  Create your FujiMod install folder with an initial Git commit, so you can use
                  tools like git diff to see everything that’s changed. The camera itself doesn’t
                  create commits.
                </p>
              </Note>
              <label class="sd-git">
                <FileRow
                  name=".git"
                  folder
                  dim={!git()}
                  check={<Check label="Track changes with Git" checked={git()} onChange={setGit} />}
                  after={<span class="sd-git-label">Track changes with Git</span>}
                />
              </label>
              <Note>
                <p>
                  The settings from original Fujifilm firmware features remain stored on your
                  camera’s internal storage. All FujiMod settings are stored in this JSON file. Why?
                  We want to avoid touching your internal storage as much as possible. It also makes
                  it easier for anyone to back up, restore, or edit their settings on the computer.
                </p>
              </Note>
              <FileRow name="SETTINGS.JSN" />
              <Note>
                <p>
                  Custom settings, aka recipes, are also stored as JSON files. You can have{' '}
                  {recipeCapacity} of them. 7 can be accessed at a time from the Q menu. You swap
                  them in the Recipes menu like Pokémon at the Pokémon Center PC.
                </p>
              </Note>
              <OptionalFolder
                name="RECIPES"
                files={inFolder('RECIPES')}
                excluded={excluded()}
                setExcluded={setExcluded}
                dim={false}
              />
              <Note>
                <p>
                  You can use custom film simulations to take more in-depth control of how your
                  images look. Film simulation configuration is passed directly to your camera’s
                  image signal processor chip.
                </p>
              </Note>
              <OptionalFolder
                name="FILMS"
                files={inFolder('FILMS')}
                excluded={excluded()}
                setExcluded={setExcluded}
                dim={false}
              />
              <Note>
                <p>
                  FujiMod implements a concept known as “atomic updates” (similar to blue-green
                  deploys). When you download a new version of FujiMod (B), we keep the previous
                  version (A) around until another update arrives.
                </p>
              </Note>
              <Folder name="SYSTEM" dim={previewing()}>
                <For each={shown()} keyed={(camera) => camera.id}>
                  {(camera) => (
                    <>
                      <Note>
                        <p>
                          A.JSN and B.JSN each list one FujiMod version’s files by their content
                          hash. {sdBootV2For(camera().processor)?.stage2 ?? 'Stage 2'} starts the newer
                          one and counts its boot attempts in STATE.JSN. A version that fails to
                          start twice is skipped for the other.
                        </p>
                      </Note>
                      <Folder name={camera().id} dim={previewing()}>
                        <FileRow name="A.JSN" dim={previewing()} />
                        <FileRow name="STATE.JSN" dim={previewing()} />
                      </Folder>
                    </>
                  )}
                </For>
                <Note>
                  <p>
                    Files are de-duplicated between FujiMod versions by storing them named based on
                    the file contents in this directory.
                  </p>
                </Note>
                <Folder name="STORE" dim={previewing()}>
                  <For each={store()} keyed={([name]) => name}>
                    {(entry) => (
                      <FileRow
                        name={entry()[0]}
                        detail={entry()[1].paths.join(', ')}
                        bytes={entry()[1].bytes}
                      />
                    )}
                  </For>
                </Folder>
              </Folder>
            </Folder>
          </div>
          <div class="sd-actions">
            <button
              type="button"
              class="sd-download-button"
              disabled={!chosen().length || pending() || !!problem() || busy()}
              onClick={() => void download()}
            >
              <Icon name="download" />
              {busy() ? progress() || 'Preparing…' : 'Download'}
              <Show when={!busy() && chosen().length && !pending() && !problem()}>
                <span>{formatBytes(plannedBytes(plan().files))}</span>
              </Show>
            </button>
            <Show when={problem() || error()}>
              <p class="sd-problem" role="alert">
                {error() || problem()}
              </p>
            </Show>
          </div>
        </div>
      </section>
    </Show>
  );
}
