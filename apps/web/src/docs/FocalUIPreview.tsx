import { Show } from 'solid-js';
import type { FocalPreview } from './sdk.generated';

/** A FocalUI preview running in Wasm, scaled to the surrounding column. */
export default function FocalUIPreview(props: { preview: FocalPreview }) {
  return (
    <figure class="sdk-preview">
      <h2 class="sdk-preview-title">{props.preview.title}</h2>
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
      <Show when={props.preview.description}>
        <figcaption>{props.preview.description}</figcaption>
      </Show>
    </figure>
  );
}
