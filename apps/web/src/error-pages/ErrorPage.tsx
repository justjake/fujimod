import { onSettled } from 'solid-js';
import { SiteFrame } from '../ui/SiteFrame';
import { manualRoot } from '../docs/manual-routes';
import type { ErrorStatus } from './routes';
import './error-page.css';

const headings = {
  404: 'Page not found',
  500: 'Something went wrong',
  503: 'Temporarily unavailable',
};

/** The page for a missing address, a failure or an outage. It is part of the
 * main bundle so it still renders when a page's own code fails to load. */
export function ErrorPage(props: { status: ErrorStatus }) {
  return (
    <SiteFrame class="error-page">
      <article data-status={props.status}>
        <h1>{headings[props.status]}</h1>
        <p>
          {props.status === 404 ? (
            <>
              <a href="/">Home</a> · <a href={manualRoot}>Manual</a>
            </>
          ) : (
            <>
              <button type="button" onClick={() => location.reload()}>
                Try again
              </button>{' '}
              · <a href="/">Home</a>
            </>
          )}
        </p>
      </article>
    </SiteFrame>
  );
}

/** Shown when a page fails to render or its code fails to load. A page that
 * fails while hydrating leaves its server-rendered markup behind; remove it. */
export function CrashPage(props: { error: unknown }) {
  console.error(props.error);
  onSettled(() => {
    for (const shell of document.querySelectorAll('body > .shell:not(.error-page)')) shell.remove();
  });
  return <ErrorPage status={500} />;
}
