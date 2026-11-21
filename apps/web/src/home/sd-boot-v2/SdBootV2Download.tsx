import { For, Show, createEffect, createMemo, createSignal } from 'solid-js';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import { formatBytes } from '@fujimod/emulator-ui/ui/file-list';
import { downloadBlob } from '@fujimod/emulator-ui/ui/download';
import { reportDownload } from '../../downloads/report.ts';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import type { Firmware } from '../../catalog';
import type { LoadedRelease } from '../../patch/release';
import { loadOtaRelease, type SdBootV2Release } from '../../ota/release';
import {
  sdBootV2For,
  sdBootV2Plan,
  conflicting,
  plannedBytes,
  offered,
  recipeCatalog,
  releaseCameras,
  type CardCamera,
} from './plan';
import { buildCard } from './build';
import { InstallSteps } from '../../install/InstallSteps';
import { InstallDialog } from '../../install/InstallDialog';
import './sd-card.css';

export function SdBootV2Download(props: { release?: LoadedRelease; catalog: readonly Firmware[] }) {
  const card = () => props.release?.sd_card_files ?? [];
  const cameras = createMemo(() =>
    props.release ? releaseCameras(props.release.firmwares, props.catalog, card()) : [],
  );
  const [chosenIds, setChosenIds] = createSignal<ReadonlySet<string>>(new Set());
  const [excluded, setExcludedPaths] = createSignal<ReadonlySet<string>>(new Set());
  const [installing, setInstalling] = createSignal(false);
  const [releases, setReleases] = createSignal<ReadonlyMap<string, SdBootV2Release | Error>>(
    new Map(),
  );
  const [busy, setBusy] = createSignal(false);
  const [progress, setProgress] = createSignal('');
  const [error, setError] = createSignal('');

  const chosen = createMemo(() => cameras().filter((camera) => chosenIds().has(camera.id)));
  const blocked = createMemo(() => conflicting(cameras(), chosen(), card()));
  const processors = createMemo(() => [...new Set(chosen().map((camera) => camera.processor))]);
  const rootFiles = createMemo(() => [
    ...new Set(
      processors().flatMap((processor) => {
        const boot = sdBootV2For(processor);
        return boot ? ['FFBOOT.BIN', 'FFSYS.BIN', boot.stage2] : [];
      }),
    ),
  ]);
  const recipes = createMemo(() => {
    const supported = chosen().length ? chosen() : cameras();
    return recipeCatalog(card()).filter((recipe) => offered(recipe, supported));
  });
  const included = createMemo(
    () => recipes().filter((recipe) => !excluded().has(recipe.path)).length,
  );
  const [recipeToggle, setRecipeToggle] = createSignal<HTMLInputElement>();
  createEffect(
    () => [recipeToggle(), included() > 0 && included() < recipes().length] as const,
    ([input, mixed]) => {
      if (input) input.indeterminate = mixed;
    },
  );
  const loaded = createMemo(() => {
    const map = new Map<string, SdBootV2Release>();
    for (const [id, value] of releases()) if (!(value instanceof Error)) map.set(id, value);
    return map;
  });
  const plan = createMemo(() =>
    sdBootV2Plan({ cameras: chosen(), card: card(), releases: loaded(), excluded: excluded() }),
  );
  const pending = () => chosen().some((camera) => !releases().has(camera.id));
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
  let preselected = false;
  createEffect(cameras, (list) => {
    if (preselected || !list.length) return;
    preselected = true;
    const x70 = list.find((camera) => camera.model === 'X70');
    if (x70) choose(x70, true);
  });
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
        progress: (done, total) => setProgress(`${formatBytes(done)} of ${formatBytes(total)}`),
      });
      downloadBlob(zip, `fujimod-sd-card-${release.release}.zip`);
      reportDownload({ release: release.release, kind: 'sd-card', cameras: chosen().length });
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
    <section class="home-download" id="download" aria-label="SD card download">
      <div class="home-sd-card">
        <img src="/brand.png" alt="Fujimod" width="88" height="22" />
        <p>
          Fujimod lives on your SD card. Running Fujimod does not change the firmware on your
          camera’s built-in storage.
        </p>
        <fieldset>
          <legend>Cameras</legend>
          <Show
            when={cameras().length}
            fallback={<p role="status">No SD card downloads available.</p>}
          >
            <For each={cameras()} keyed={(camera) => camera.id}>
              {(camera) => (
                <label class="sd-camera">
                  <input
                    type="checkbox"
                    checked={chosenIds().has(camera().id)}
                    disabled={blocked().has(camera().id)}
                    onChange={(event) => choose(camera(), event.currentTarget.checked)}
                  />
                  <span>{camera().model}</span>
                  <small>Requires firmware {camera().version}</small>
                </label>
              )}
            </For>
          </Show>
        </fieldset>
        <Show when={recipes().length}>
          <fieldset aria-label="Recipes">
            <label>
              <input
                type="checkbox"
                checked={included() === recipes().length}
                ref={setRecipeToggle}
                onChange={(event) =>
                  setExcluded(
                    recipes().map((recipe) => recipe.path),
                    !event.currentTarget.checked,
                  )
                }
              />
              <span>Recipes</span>
              <small>{recipes().length}</small>
            </label>
            <div class="home-recipe-list">
              <For each={recipes()}>
                {(recipe) => (
                  <label>
                    <input
                      type="checkbox"
                      checked={!excluded().has(recipe.path)}
                      onChange={(event) => setExcluded([recipe.path], !event.currentTarget.checked)}
                    />
                    <span class="home-recipe">
                      <span>{recipe.title ?? recipe.path.split('/').at(-1)}</span>
                      <Show when={recipe.author}>
                        {(author) => (
                          <Show when={recipe.source} fallback={<small>{author()}</small>}>
                            {(source) => (
                              <a
                                class="sd-source"
                                href={source()}
                                target="_blank"
                                rel="noopener noreferrer"
                              >
                                {author()}
                              </a>
                            )}
                          </Show>
                        )}
                      </Show>
                    </span>
                  </label>
                )}
              </For>
            </div>
          </fieldset>
        </Show>
      </div>
      <button
        type="button"
        class="home-download-button"
        disabled={!chosen().length}
        onClick={() => setInstalling(true)}
      >
        Download
      </button>
      <div class="home-download-disclaimer">
        <p>
          Please read <a href="/safety">safety</a> so you can make an informed decision about using
          Fujimod.
        </p>
        <p>
          Fujimod is not approved or endorsed by Fujifilm in any way, and using it will probably
          void your warranty. We are not responsible for any damages to your camera.
        </p>
      </div>
      <Show when={installing()}>
        <InstallDialog
          title="FujiMod SD boot v2"
          identity={`${chosen()
            .map((camera) => `${camera.model} ${camera.version}`)
            .join(' • ')} • ${props.release?.tag ?? ''}`}
          busy={busy()}
          close={() => setInstalling(false)}
        >
          <InstallSteps
            kind="sd-boot-v2"
            cameras={chosen().map((camera) => ({
              firmwareId: camera.id,
              name: `${camera.model} version ${camera.version}`,
            }))}
            files={['FUJIMOD', ...rootFiles()]}
            loader={processors()
              .map((processor) => sdBootV2For(processor)?.stage2)
              .find(Boolean)}
            download={
              <>
                <button
                  type="button"
                  class="mod-download"
                  disabled={pending() || !!problem() || busy()}
                  onClick={() => void download()}
                >
                  <Icon name="download" />
                  {busy() ? progress() || 'Preparing…' : 'Download SD card files'}
                  <Show when={!busy() && !pending() && !problem()}>
                    <span class="mod-download-size">{formatBytes(plannedBytes(plan().files))}</span>
                  </Show>
                </button>
                <Show when={problem() || error()}>
                  <p class="firmware-error" role="alert">
                    {error() || problem()}
                  </p>
                </Show>
              </>
            }
          />
        </InstallDialog>
      </Show>
    </section>
  );
}
