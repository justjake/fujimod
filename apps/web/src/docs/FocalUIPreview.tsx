import { Show, createSignal } from 'solid-js';
import type { FocalPreview } from './sdk.generated';
import { highlightC } from './Markdown';

/** A FocalUI preview running in Wasm, scaled to the surrounding column. */
export default function FocalUIPreview(props: { preview: FocalPreview }) {
  const [view, setView] = createSignal<'preview' | 'source'>('preview');
  return (
    <figure class="sdk-preview">
      <h2 class="sdk-preview-title">{props.preview.title}</h2>
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
      <Show
        when={view() === 'source'}
        fallback={
          <div
            class="sdk-preview-screen"
            style={{ 'aspect-ratio': `${props.preview.width} / ${props.preview.height}` }}
          >
            <iframe
              src={props.preview.page}
              title={props.preview.title}
              width={props.preview.width}
              height={props.preview.height}
              loading="lazy"
            />
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
