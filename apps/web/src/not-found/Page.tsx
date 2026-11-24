import { SiteFrame } from '../ui/SiteFrame';
import { manualRoot } from '../docs/manual-routes';
import './not-found.css';

/** Served as 404.html for every address the site has no page for. */
export default function NotFound() {
  return (
    <SiteFrame class="not-found">
      <article>
        <h1>Page not found</h1>
        <p>
          <a href="/">Home</a> · <a href={manualRoot}>Manual</a>
        </p>
      </article>
    </SiteFrame>
  );
}
