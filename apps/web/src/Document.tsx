import { NoHydration, Show } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { getRequestEvent, HydrationScript } from '@solidjs/web';
import { pageMetadata, siteOrigin } from './page-metadata';
import { featureMedia } from './home/feature-media';
export default function Document(props: { children: JSX.Element; developmentEntry?: string }) {
  const path = new URL(getRequestEvent()?.request.url ?? window.location.href).pathname;
  const bug = /^\/bug(?:\/|$)/.test(path);
  const metadata = pageMetadata(path);
  const preview = path === '/' ? featureMedia()('shape_aspect_item') : undefined;
  return (
    <html lang="en">
      <head>
        {/* The client never walks <head>: browser extensions add nodes to it before
            the page hydrates, and a positional walk over them stops hydration. */}
        <NoHydration>
          <meta charset="UTF-8" />
          <meta name="viewport" content="width=device-width,initial-scale=1" />
          <base href={import.meta.env.BASE_URL} />
          <meta name="description" content={metadata.description} />
          <link rel="canonical" href={metadata.canonical} />
          <meta property="og:type" content="website" />
          <meta property="og:site_name" content="FujiMod" />
          <meta property="og:title" content={metadata.title} />
          <meta property="og:description" content={metadata.description} />
          <meta property="og:url" content={metadata.canonical} />
          <meta name="twitter:card" content={preview ? 'summary_large_image' : 'summary'} />
          <meta name="twitter:title" content={metadata.title} />
          <meta name="twitter:description" content={metadata.description} />
          <Show when={preview}>
            {(media) => (
              <>
                <meta
                  property="og:image"
                  content={`${siteOrigin}/artifacts/docs/${media().still}`}
                />
                <meta property="og:image:type" content="image/webp" />
                <meta property="og:image:width" content="720" />
                <meta property="og:image:height" content="480" />
                <meta
                  property="og:image:alt"
                  content="FujiMod’s 65:24 XPan crop with a 50% darkened surround."
                />
                <meta
                  name="twitter:image"
                  content={`${siteOrigin}/artifacts/docs/${media().still}`}
                />
                <meta
                  name="twitter:image:alt"
                  content="FujiMod’s 65:24 XPan crop with a 50% darkened surround."
                />
              </>
            )}
          </Show>
          <meta name="referrer" content="no-referrer" />
          <Show when={import.meta.env.DEV && !bug}>
            <meta name="fujimod-local-reports" content="/__simulator-reports" />
            <script src="/__simulator-reports.js" />
            <script src="/__browser-diagnostics.js" />
          </Show>
          <title>{metadata.title}</title>
          <link rel="icon" href="data:," />
          <HydrationScript />
          <Show when={props.developmentEntry}>
            <script type="module" src={props.developmentEntry} async />
          </Show>
        </NoHydration>
      </head>
      <body>{props.children}</body>
    </html>
  );
}
