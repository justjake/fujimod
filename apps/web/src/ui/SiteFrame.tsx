import { getRequestEvent, type JSX } from '@solidjs/web';
import { For } from 'solid-js';
import { isDevToolsPath } from '../dev-tools/routes';
import { isManualPath, manualRoot } from '../docs/manual-routes';
import './site-frame.css';

export function SiteFrame(props: { class?: string; children: JSX.Element }) {
  const path = new URL(getRequestEvent()?.request.url ?? window.location.href).pathname;
  const pages = [
    { label: 'Manual', href: manualRoot, current: isManualPath(path) },
    { label: 'Emulator', href: '/#software' },
    { label: 'Safety', href: '/safety', current: /^\/safety\/?$/.test(path) },
    { label: 'Dev Tools', href: '/dev-tools/', current: isDevToolsPath(path) },
    { label: 'Recovery', href: '/recovery/', current: /^\/recovery\/?$/.test(path) },
  ];

  return (
    <div class={['shell', props.class]}>
      <header class="site-header">
        <a class="site-brand" href="/">
          <img src="/brand.png" alt="Fujimod" width="110" height="28" />
        </a>
        <nav class="site-nav" aria-label="Pages">
          <For each={pages}>
            {(page) => (
              <a href={page.href} aria-current={page.current ? 'page' : undefined}>
                {page.label}
              </a>
            )}
          </For>
        </nav>
        <nav class="site-external" aria-label="Community">
          <a href="https://github.com/justjake/fujimod">GitHub</a>
          <a href="https://discord.gg/jWvFPakw7X">Discord</a>
        </nav>
      </header>
      {props.children}
    </div>
  );
}
