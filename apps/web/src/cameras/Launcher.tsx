import { For, Show, createSignal } from 'solid-js';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import type { CameraSupport } from '../home/state';
import bodies from '../../data/cameras.json';
import type { CameraRecord, LibraryFirmware } from './store';
import type { Firmware } from '../catalog';
import type { LoadedRelease } from '../patch/release';
import type { FirmwareDownloadFormat } from '../firmware-download';
import type { FirmwareLibrary } from './firmware-library';
import { FirmwarePicker } from './FirmwarePicker';
import { DownloadModal } from './DownloadModal';
import { latestCamera } from './selection';
import './launcher.css';

const announced = new Map(bodies.map((body) => [body.name, body.announced]));
/** Newest announcement first, then by model name. */
export function byAnnouncement(a: Firmware, b: Firmware) {
  return (
    (announced.get(b.model) ?? '').localeCompare(announced.get(a.model) ?? '') ||
    a.model.localeCompare(b.model, 'en', { numeric: true })
  );
}
/** FujiMod bodies first, then newest announcement first. */
export function cameraOrder(a: CameraSupport, b: CameraSupport) {
  return Number(b.mod) - Number(a.mod) || byAnnouncement(a.firmware, b.firmware);
}
const rows = [
  {
    id: 'installable',
    title: 'Download FujiMod installable',
    subtitle: 'Includes SD boot version',
    icon: 'flash',
  },
  {
    id: 'sd-boot-v1',
    title: 'Download FujiMod SD boot v1',
    subtitle: 'Dual boot from an SD card',
    icon: 'sd-card',
  },
  {
    id: 'mod',
    title: 'Run FujiMod in your browser',
    subtitle: 'FujiMod on an emulated camera',
    icon: 'play',
  },
  {
    id: 'stock',
    title: 'Run unmodified in your browser',
    subtitle: 'Official firmware, emulated',
    icon: 'play',
  },
] as const;
type Row = (typeof rows)[number];

export function CameraLauncher(props: {
  ready: boolean;
  cameras: CameraRecord[];
  supported: CameraSupport[];
  library: FirmwareLibrary;
  release?: LoadedRelease;
  busy: boolean;
  message: string;
  error: boolean;
  run: (firmware: LibraryFirmware, modEnabled: boolean, fresh: boolean) => void;
  download: (firmware: LibraryFirmware, format: FirmwareDownloadFormat) => Promise<void>;
}) {
  const [modal, setModal] = createSignal<{
    firmware: LibraryFirmware;
    format: FirmwareDownloadFormat;
    release: string;
    updateFilename?: string;
  }>();
  const [downloading, setDownloading] = createSignal(false);
  const [downloadError, setDownloadError] = createSignal('');
  const [fetching, setFetching] = createSignal(false);
  const [fetchError, setFetchError] = createSignal('');
  const choices = () => [...props.supported].sort(cameraOrder);
  const selected = () => props.library.selected() ?? choices()[0]?.firmware.sha256;
  const chosen = () => props.library.entries().find((entry) => entry.sha256 === selected());
  const target = () =>
    props.supported.find((entry) => entry.firmware.sha256 === selected())?.firmware;
  const busy = () => props.busy || props.library.busy() || fetching();
  const support = () => props.supported.find((entry) => entry.firmware.sha256 === selected());
  const patch = () => props.release?.firmwares.find((entry) => entry.sha256 === selected());
  const label = () =>
    target() ? `${target()!.model} • ${target()!.version}` : chosen()?.name.replace(' · ', ' • ');
  const modRelease = () =>
    props.release ? `FujiMod ${props.release.tag.replace(/^v/, '')}` : 'FujiMod';
  const available = (row: Row) => {
    if (!target()) return false;
    switch (row.id) {
      case 'installable':
        return !!patch()?.downloads.sd_boot_v1_installable;
      case 'sd-boot-v1':
        return !!patch()?.downloads.sd_boot_v1;
      case 'mod':
        return !!support()?.emu && !!support()?.mod;
      case 'stock':
        return !!support()?.emu;
    }
  };
  const recent = (row: Row) =>
    chosen() && available(row) && (row.id === 'mod' || row.id === 'stock')
      ? latestCamera(props.cameras, chosen()!.sha256, row.id === 'mod')
      : undefined;
  const subtitle = (row: Row) => {
    if (!target()) return row.subtitle;
    if (!available(row)) return 'Not available for this camera yet';
    switch (row.id) {
      case 'installable':
        return `${label()} • Includes SD boot version`;
      case 'sd-boot-v1':
        return `${label()} base firmware`;
      case 'mod':
        return `${label()} with ${modRelease()}`;
      case 'stock':
        return `${label()} official firmware`;
    }
  };
  function activate(row: Row, fresh = false) {
    if (!available(row) || busy()) return;
    const launch = (firmware: LibraryFirmware) => {
      if (row.id === 'mod' || row.id === 'stock') props.run(firmware, row.id === 'mod', fresh);
      else {
        setDownloadError('');
        setModal({
          firmware,
          format: row.id,
          release: modRelease(),
          updateFilename: patch()?.update_filename,
        });
      }
    };
    setFetchError('');
    const saved = chosen();
    if (saved) {
      launch(saved);
      return;
    }
    const firmware = target();
    if (!firmware) return;
    setFetching(true);
    void props.library
      .get(firmware)
      .then(launch)
      .catch((error: unknown) => {
        setFetchError(shownError(error));
      })
      .finally(() => setFetching(false));
  }
  async function download() {
    const target = modal();
    if (!target || downloading()) return;
    setDownloading(true);
    setDownloadError('');
    try {
      await props.download(target.firmware, target.format);
    } catch (error) {
      if (modal() === target) setDownloadError(shownError(error));
    } finally {
      setDownloading(false);
    }
  }
  return (
    <div class={{ launcher: true, 'launcher-pending': !props.ready }}>
      <section class="launcher-actions" aria-label="Camera actions">
        <label class="launcher-camera">
          Camera
          <select
            aria-label="Camera"
            value={selected() ?? ''}
            disabled={busy()}
            onChange={(event) => {
              props.library.select(event.currentTarget.value);
              setFetchError('');
            }}
          >
            <For each={choices()}>
              {({ firmware }) => (
                <option value={firmware.sha256}>
                  {firmware.model} · {firmware.version}
                </option>
              )}
            </For>
          </select>
        </label>
        <div class="release-actions">
          <For each={rows}>
            {(row) => (
              <div class={{ 'release-action': true, unavailable: !available(row) }}>
                <button
                  class="release-hit"
                  aria-label={row.title}
                  aria-describedby={'release-subtitle-' + row.id}
                  disabled={busy() || !available(row)}
                  onClick={() => activate(row)}
                />
                <div class="launcher-row">
                  <span class={'release-icon ' + row.id}>
                    <Icon name={row.icon} size={18} />
                  </span>
                  <span class="launcher-row-text">
                    <strong>{row.title}</strong>
                    <span
                      id={'release-subtitle-' + row.id}
                      class={{ 'release-unavailable': !!chosen() && !available(row) }}
                    >
                      {subtitle(row)}
                      <Show when={recent(row)}>
                        {' • '}
                        <button
                          class="start-fresh"
                          disabled={busy()}
                          onClick={() => {
                            activate(row, true);
                          }}
                        >
                          Start fresh
                        </button>
                      </Show>
                    </span>
                  </span>
                  <Show when={available(row)}>
                    <Icon name="forward" size={16} />
                  </Show>
                </div>
              </div>
            )}
          </For>
        </div>
      </section>
      <div class="launcher-firmware">
        <FirmwarePicker
          library={props.library}
          inputId="firmware-file"
          disabled={props.busy}
          heading="Base firmware"
        />
        <span
          class={{ 'camera-storage-status': true, error: props.error || !!fetchError() }}
          role="status"
          aria-live="polite"
        >
          {fetching() ? 'Downloading firmware…' : fetchError() || props.message}
        </span>
      </div>
      <Show when={modal()}>
        {(target) => (
          <DownloadModal
            format={target().format}
            firmware={target().firmware}
            release={target().release}
            updateFilename={target().updateFilename}
            busy={downloading()}
            error={downloadError()}
            download={() => void download()}
            close={() => setModal(undefined)}
          />
        )}
      </Show>
    </div>
  );
}
