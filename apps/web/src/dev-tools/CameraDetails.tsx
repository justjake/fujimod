import { Show, createMemo, createSignal } from 'solid-js';
import { catalog } from 'fujimod';
import type { Camera, CameraModel } from './camera-model';
import { cameraState } from './CameraStrip';
import { SegmentedControl } from '@fujimod/emulator-ui/ui/SegmentedControl';
import { firmwareVersion } from './firmware-version';

export function CameraDetails(props: { camera: Camera; model: CameraModel }) {
  const [copied, setCopied] = createSignal(false);
  const [sharingBusy, setSharingBusy] = createSignal(false);
  const [sharingError, setSharingError] = createSignal('');
  const state = () => cameraState(props.camera);
  const identity = () => props.camera.identity;
  const firmware = createMemo(() => {
    const camera = identity();
    return camera?.product && camera.internalVersion
      ? firmwareVersion(camera, catalog.firmwareVersions())
      : undefined;
  });
  // "FUJIFILM X100V", without repeating a product that already names its maker.
  const product = () => {
    const maker = identity()?.manufacturer ?? identity()?.vendor;
    const name = identity()?.product;
    if (!name) return maker;
    return maker && !name.toUpperCase().startsWith(maker.toUpperCase()) ? `${maker} ${name}` : name;
  };
  async function setSharing(on: boolean) {
    if (on === Boolean(props.camera.sharing)) return;
    setSharingBusy(true);
    setSharingError('');
    try {
      await props.model.setSharing(props.camera.id, on);
    } catch (error) {
      setSharingError(error instanceof Error ? error.message : String(error));
    } finally {
      setSharingBusy(false);
    }
  }
  async function copyLink() {
    const link = props.camera.sharing?.link;
    if (!link) return;
    await navigator.clipboard.writeText(link);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }
  return (
    <section class="wb-camera" aria-labelledby="wb-camera-name">
      <header class="wb-camera-head">
        <h2 id="wb-camera-name">{props.camera.name}</h2>
        <span class={`wb-pill ${state().tone}`}>
          <i class={`wb-dot ${state().tone}`} aria-hidden="true" />
          <span class="wb-state">{state().label}</span>
        </span>
      </header>
      <Show when={identity()}>
        <dl class="wb-kv">
          <Show when={product()}>
            <dt>Product</dt>
            <dd>{product()}</dd>
          </Show>
          <Show when={identity()?.internalVersion}>
            <dt>Firmware</dt>
            <dd>
              {firmware()?.publicVersion ?? 'Unknown'}
              <span class="wb-muted"> · internal {identity()!.internalVersion}</span>
              <Show when={identity()?.framework}>
                <span class="wb-muted"> · framework {identity()!.framework}</span>
              </Show>
            </dd>
          </Show>
          <Show when={identity()?.serial}>
            <dt>Serial</dt>
            <dd class="wb-serial" title={identity()!.serial}>
              {identity()!.serial}
            </dd>
          </Show>
        </dl>
      </Show>
      <Show when={firmware()?.outdated && props.camera.transport !== 'simulated'}>
        <div class="wb-firmware-update" role="status">
          <p>
            {firmware()?.publicVersion
              ? 'Newer firmware is available.'
              : 'Unknown firmware version appears to be outdated.'}
          </p>
          <p class="wb-muted">Latest known release: {firmware()!.latest.version}</p>
          <a
            class="wb-btn"
            href={firmware()!.latest.download_url}
            target="_blank"
            rel="noopener noreferrer"
          >
            Update to latest official firmware ↗
          </a>
        </div>
      </Show>
      <Show when={props.camera.remoteActivity?.progress}>
        {(progress) => (
          <div
            class="wb-progress"
            role="progressbar"
            aria-label={progress().label}
            aria-valuemin={0}
            aria-valuemax={progress().total}
            aria-valuenow={progress().done}
          >
            <i
              style={{
                width: `${((progress().total > 0 ? Math.min(1, progress().done / progress().total) : 0) * 100).toFixed(1)}%`,
              }}
            />
          </div>
        )}
      </Show>
      <Show when={props.camera.problem}>
        {(problem) => (
          <div class="wb-problem" role="alert">
            <span>{problem()}</span>
            <button class="wb-text" onClick={() => props.model.dismissProblem(props.camera.id)}>
              Dismiss
            </button>
          </div>
        )}
      </Show>
      <Show when={props.camera.transport === 'usb' && props.camera.present}>
        <div class="wb-camera-row">
          <span class="wb-row-label">Sharing</span>
          <SegmentedControl
            label="Sharing"
            options={[
              { value: false, label: 'Off' },
              { value: true, label: 'On' },
            ]}
            value={Boolean(props.camera.sharing)}
            disabled={sharingBusy()}
            onSelect={(on) => void setSharing(on)}
          />
        </div>
        <Show when={props.camera.sharing}>
          {(sharing) => (
            <div class="wb-share">
              <span class="wb-share-link" title={sharing().link}>
                {sharing().link}
              </span>
              <button class="wb-btn wb-copy" onClick={() => void copyLink()}>
                {copied() ? 'Copied' : 'Copy link'}
              </button>
              <Show when={sharing().peers.size}>
                <span class="wb-muted wb-share-peers">
                  {sharing().peers.size} {sharing().peers.size === 1 ? 'controller' : 'controllers'}{' '}
                  connected
                </span>
              </Show>
            </div>
          )}
        </Show>
        <Show when={sharingError()}>
          <p class="wb-status" role="status">
            {sharingError()}
          </p>
        </Show>
      </Show>
    </section>
  );
}
