import { SiteFrame } from '../../ui/SiteFrame';

/** Prototypes of camera screens, drawn with the design library in src/design. */
const PROTOTYPES = [
  { path: '/docs/design/custom-settings-editor/', name: 'Custom settings editor', summary: 'Edit a custom setting over live view, with the film editor over it.' },
];

/** /docs/design/: the screen prototypes. */
export default function DesignIndex() {
  return (
    <SiteFrame class="design-index">
      <main class="mx-auto max-w-[720px] px-4 py-8">
        <h1 class="mb-6 text-[28px] font-semibold">Design</h1>
        <ul class="flex flex-col gap-4">
          {PROTOTYPES.map((p) => (
            <li>
              <a class="text-[19px] font-semibold underline" href={p.path}>
                {p.name}
              </a>
              <p class="opacity-70">{p.summary}</p>
            </li>
          ))}
        </ul>
      </main>
    </SiteFrame>
  );
}
