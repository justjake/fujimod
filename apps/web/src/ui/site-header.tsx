/** The site header: the brand links home, then the page links and the source repository. */
export function SiteHeader(props: {
  /** Path from the current page to the site root, such as `''` or `'../'`. */
  root: string;
  current?: 'dev-tools' | 'recovery';
}) {
  const current = (page: 'dev-tools' | 'recovery') =>
    props.current === page ? 'page' : undefined;
  return (
    <header>
      <a href={props.root || './'} aria-label="Fujimod">
        <img src={`${props.root}brand.png`} alt="Fujimod" width="110" height="28" />
      </a>
      <nav class="site-nav" aria-label="Pages">
        <a href={`${props.root}dev-tools/`} aria-current={current('dev-tools')}>
          Dev Tools
        </a>
        <a href={`${props.root}recovery/`} aria-current={current('recovery')}>
          Recovery
        </a>
      </nav>
      <div class="meta build-info">
        <a href="https://github.com/justjake/fujimod">github.com/justjake/fujimod</a>
      </div>
    </header>
  );
}
