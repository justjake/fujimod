import { For, Show, createSignal, onSettled } from 'solid-js';
import type { Timeline } from 'animejs';
import { cameras, type Camera } from './cameras';
import { buildScene, initialRig, type Rig } from './scene';
import { beatAt, stepTimeline, steps } from './steps';
import '../home/site.css';
import './setup.css';

type Link =
  | { state: 'idle' }
  | { state: 'choosing' }
  | { state: 'connected'; name: string }
  | { state: 'failed'; message: string };

/** Service-mode setup wizard (SETUP-WIZARD.md). */
export default function Setup() {
  const [camera, setCamera] = createSignal<Camera>(cameras[0]);
  const [step, setStep] = createSignal(0);
  const [beat, setBeat] = createSignal(0);
  const [link, setLink] = createSignal<Link>({ state: 'idle' });
  const supported = typeof window !== 'undefined' && isSecureContext && 'usb' in navigator;
  let svg: SVGSVGElement | undefined;
  let stage: HTMLDivElement | undefined;
  let rig: Rig = initialRig(cameras[0]);
  let draw: ((rig: Rig, aspect: number) => void) | undefined;
  let timeline: Timeline | undefined;
  const still =
    typeof window !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches;

  // Solid batches signal writes, so callers pass the step and camera they just set.
  function play(index = step(), body = camera()) {
    timeline?.pause();
    timeline = stepTimeline(steps[index].id, body, rig);
    if (still) timeline.seek(timeline.duration, true);
  }
  function show(index: number) {
    const next = Math.max(0, Math.min(steps.length - 1, index));
    if (steps[next].id === 'connect' && supported) void loadUsb().catch(() => {});
    setStep(next);
    setBeat(0);
    play(next);
  }
  function pick(next: Camera) {
    setCamera(next);
    rig = initialRig(next);
    if (svg) draw = buildScene(svg, next);
    play(step(), next);
  }

  // Loaded when the connect step opens, so the button goes straight to Chrome's chooser
  // (it only opens from a recent click).
  let usbModule: Promise<typeof import('fujimod')> | undefined;
  const loadUsb = () =>
    (usbModule ??= import('fujimod').then(async (m) => {
      await m.initialize();
      return m;
    }));
  async function connect() {
    setLink({ state: 'choosing' });
    try {
      const { usb } = await loadUsb();
      const device = await usb.requestDevice(usb.DeviceFilter.Diagnostic);
      if (!device) return setLink({ state: 'idle' });
      setLink({ state: 'connected', name: device.productName || 'Fujifilm camera' });
    } catch (cause) {
      setLink({ state: 'failed', message: cause instanceof Error ? cause.message : String(cause) });
    }
  }

  onSettled(() => {
    if (svg) draw = buildScene(svg, camera());
    let frame = 0,
      last = performance.now();
    const tick = (now: number) => {
      rig.phase += (now - last) / 1100;
      last = now;
      const box = stage?.getBoundingClientRect();
      if (box) draw?.(rig, box.width / Math.max(1, box.height));
      if (timeline) {
        const b = beatAt(timeline);
        if (b !== beat()) setBeat(b);
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    play();
    // For screenshot checks: jump the current step to a moment, in milliseconds.
    const w = window as Window & { setupSeek?: (ms: number) => void; setupRig?: () => Rig };
    w.setupSeek = (ms) => {
      timeline?.pause();
      timeline?.seek(ms, true);
    };
    w.setupRig = () => ({ ...rig });
    (w as Window & { setupChooser?: () => void }).setupChooser = () =>
      setLink({ state: 'choosing' });
    return () => {
      cancelAnimationFrame(frame);
      timeline?.pause();
    };
  });

  const current = () => steps[step()];
  const guard = () =>
    ({
      known:
        'Exit protection: known for this camera, and the page will arm it as soon as the camera connects.',
      candidate:
        'Exit protection: a candidate exists for this camera but is not reviewed, so the page leaves it off.',
      none: 'Exit protection: none is known for this camera. The battery exit in step 4 is the way out.',
    })[camera().guard];

  return (
    <main class="setup">
      <header class="setup-head">
        <a href="../" class="setup-back">
          FujiMod
        </a>
        <h1>Set up USB access</h1>
        <div class="setup-cameras" role="radiogroup" aria-label="Camera">
          <For each={cameras}>
            {(c) => (
              <button
                role="radio"
                aria-checked={camera().id === c.id ? 'true' : 'false'}
                class={{ on: camera().id === c.id }}
                onClick={() => pick(c)}
              >
                {c.name}
              </button>
            )}
          </For>
        </div>
      </header>

      <Show when={!camera().entry.reviewed}>
        <p class="setup-note">
          Provisional for the {camera().name}. {camera().entry.note}
        </p>
      </Show>

      <ol class="setup-steps">
        <For each={steps}>
          {(s, i) => (
            <li class={{ on: step() === i(), done: step() > i() }}>
              <button onClick={() => show(i())}>
                <span class="n">{i() + 1}</span>
                {s.title}
              </button>
            </li>
          )}
        </For>
      </ol>

      <div class="setup-stage" ref={(element) => (stage = element)}>
        <svg
          ref={(element) => (svg = element)}
          class="setup-art"
          role="img"
          aria-label={current().beats[beat()]}
        />
      </div>

      <section class="setup-panel">
        <p class="setup-caption" aria-live="polite">
          {current().beats[beat()] ?? current().beats[0]}
        </p>
        <Show when={current().beats.length > 1}>
          <div class="setup-beats" aria-hidden="true">
            <For each={current().beats}>{(_, i) => <span class={{ on: beat() === i() }} />}</For>
          </div>
        </Show>

        <Show when={current().id === 'enter' && camera().entry.reviewed}>
          <p class="setup-detail">{camera().entry.note}</p>
        </Show>

        <Show when={current().id === 'connect'}>
          <Show
            when={supported}
            fallback={
              <p class="setup-detail warn">
                This needs Chrome or Edge on a computer: other browsers can't talk to USB devices.
              </p>
            }
          >
            <Show
              when={link().state === 'connected'}
              fallback={
                <button
                  class="setup-primary"
                  disabled={link().state === 'choosing'}
                  onClick={connect}
                >
                  Connect camera over USB
                </button>
              }
            >
              <p class="setup-detail ok">Connected to {(link() as { name: string }).name}.</p>
            </Show>
            <Show when={link().state === 'failed'}>
              <p class="setup-detail warn">{(link() as { message: string }).message}</p>
            </Show>
            <p class="setup-detail">{guard()}</p>
            <Show when={camera().guard === 'known'}>
              <p class="setup-detail dim">
                That part is not built yet, so for now nothing is armed. The battery exit in step 4
                works without it.
              </p>
            </Show>
          </Show>
        </Show>

        <Show when={current().id === 'debug'}>
          <p class="setup-detail dim">
            Turning on debug mode from the browser is not built yet. This step will do it over the
            same connection.
          </p>
        </Show>

        <div class="setup-nav">
          <button onClick={() => show(step() - 1)} disabled={step() === 0}>
            Back
          </button>
          <button onClick={() => play()}>Replay</button>
          <button
            class="setup-primary"
            onClick={() => show(step() + 1)}
            disabled={step() === steps.length - 1}
          >
            Next
          </button>
        </div>
      </section>

      <Show when={link().state === 'choosing'}>
        <div class="chooser-dim" />
        <div class="chooser-hint" role="note">
          <svg viewBox="0 0 44 56" class="chooser-arrow" aria-hidden="true">
            <path d="M22 52 V6 M8 20 L22 6 L36 20" />
          </svg>
          <p>
            <b>Select your camera above.</b> It will not be modified until you approve.
          </p>
        </div>
      </Show>
    </main>
  );
}
