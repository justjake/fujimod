import { Show, createSignal } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { KeyFace, PadFace } from '@fujimod/emulator-ui/ui/control-faces';
import type { FocalPreview } from './sdk-data';
import { highlightC } from './Markdown';

const directionKeys = new Map([
  ['up', { key: 'ArrowUp', label: 'Up' }],
  ['down', { key: 'ArrowDown', label: 'Down' }],
  ['left', { key: 'ArrowLeft', label: 'Left' }],
  ['right', { key: 'ArrowRight', label: 'Right' }],
  ['press', { key: 'Enter', label: 'MENU/OK' }],
]);

function PreviewControls(props: { sendKey: (key: string) => void }) {
  const render = (
    button: { key: string; label: string },
    className: string,
    content: JSX.Element,
  ) => (
    <button
      type="button"
      class={className}
      aria-label={button.label}
      onClick={() => props.sendKey(button.key)}
    >
      {content}
    </button>
  );
  return (
    <div class="sdk-preview-buttons" role="group" aria-label="Camera controls">
      <PadFace
        size="panel"
        part={{ kind: 'pad', marking: { face: ['MENU', 'OK'] } }}
        renderKey={(action, className, content) =>
          render(directionKeys.get(action)!, className, content)
        }
      />
      <KeyFace
        size="panel"
        part={{ kind: 'key', marking: { face: ['DISP', 'BACK'] } }}
        renderKey={(_action, className, content) =>
          render({ key: 'Escape', label: 'DISP/BACK' }, className, content)
        }
      />
    </div>
  );
}

/** A FocalUI preview running in Wasm, scaled to the surrounding column. */
export default function FocalUIPreview(props: { preview: FocalPreview }) {
  const [view, setView] = createSignal<'preview' | 'source'>('preview');
  const [showControls, setShowControls] = createSignal(false);
  let frame: HTMLIFrameElement | undefined;
  const sendKey = (key: string) => {
    frame?.contentWindow?.postMessage({ type: 'focal-preview-key', key }, window.location.origin);
  };
  return (
    <figure class="sdk-preview">
      <h2 class="sdk-preview-title">{props.preview.title}</h2>
      <div class="sdk-preview-toolbar">
        <div class="sdk-preview-tabs" role="tablist" aria-label={props.preview.title}>
          <button
            type="button"
            role="tab"
            aria-selected={view() === 'preview' ? 'true' : 'false'}
            onClick={() => {
              setView('preview');
            }}
          >
            Preview
          </button>
          <Show when={props.preview.source}>
            <button
              type="button"
              role="tab"
              aria-selected={view() === 'source' ? 'true' : 'false'}
              onClick={() => {
                setView('source');
              }}
            >
              Source
            </button>
          </Show>
        </div>
        <Show when={view() === 'preview'}>
          <button
            type="button"
            aria-expanded={showControls() ? 'true' : 'false'}
            onClick={() => {
              setShowControls((visible) => !visible);
            }}
          >
            {showControls() ? 'Hide controls' : 'Show controls'}
          </button>
        </Show>
      </div>
      <Show
        when={view() === 'source'}
        fallback={
          <div class="sdk-preview-stage">
            <div
              class="sdk-preview-screen"
              style={{ 'aspect-ratio': `${props.preview.width} / ${props.preview.height}` }}
            >
              <iframe
                ref={(element) => {
                  frame = element;
                }}
                src={props.preview.page}
                title={props.preview.title}
                width={props.preview.width}
                height={props.preview.height}
                loading="lazy"
              />
            </div>
            <Show when={showControls()}>
              <PreviewControls sendKey={sendKey} />
            </Show>
          </div>
        }
      >
        <pre class="sdk-code sdk-preview-source">
          <code>{highlightC(props.preview.source)}</code>
        </pre>
      </Show>
      <Show when={props.preview.description}>
        <figcaption>{props.preview.description}</figcaption>
      </Show>
    </figure>
  );
}
