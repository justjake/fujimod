import { For, Show, onSettled } from 'solid-js';
import { Lightbox } from 'lightbox3';
import 'lightbox3/style.css';
import TextPage, { type TextSection } from './TextPage';
import credits from './credits.generated.json';
import { OpenSource } from './OpenSource';
import './credits.css';

type Person = { name: string; source?: string; avatar?: string };

function People(props: { people: readonly Person[] }) {
  return (
    <ul class="credits-people">
      <For each={props.people}>
        {(person) => (
          <li>
            <Show when={person.source} fallback={<span>{person.name}</span>}>
              {(source) => (
                <a href={source()}>
                  <Show when={person.avatar}>
                    {(avatar) => (
                      <img
                        src={avatar()}
                        alt=""
                        width="28"
                        height="28"
                        loading="lazy"
                        crossorigin="anonymous"
                      />
                    )}
                  </Show>
                  <span>{person.name}</span>
                </a>
              )}
            </Show>
          </li>
        )}
      </For>
    </ul>
  );
}

/** Scene previews extracted from the RAFs at build (scripts/assets/scene-previews.ts). */
const previews: Record<string, string> = import.meta.glob('../../../../build/assets/scenes/*.jpg', {
  eager: true,
  query: '?url',
  import: 'default',
});
const preview = (scene: string) => {
  const url = previews[`../../../../build/assets/scenes/${scene}.jpg`];
  if (!url) throw Error(`No preview for scene ${scene}`);
  return url;
};

/** A row of each photographer's scene photographs; a photograph opens in a lightbox. */
function Photographs() {
  onSettled(() => {
    const lightbox = Lightbox.init();
    return () => lightbox.destroy();
  });
  return (
    <dl class="credits-photos">
      <For each={credits.photographers}>
        {(person) => (
          <div>
            <dt>
              <Show when={person.source} fallback={person.name}>
                {(source) => <a href={source()}>{person.name}</a>}
              </Show>
            </dt>
            <dd>
              <ul class="film-strip">
                <For each={person.photos}>
                  {(photo) => (
                    <li>
                      <a
                        href={preview(photo.scene)}
                        data-lightbox={`photos-${person.name}`}
                        data-caption={`${photo.title} · ${photo.camera}`}
                      >
                        <img
                          src={preview(photo.scene)}
                          alt={photo.title}
                          width="360"
                          height="240"
                          loading="lazy"
                        />
                      </a>
                    </li>
                  )}
                </For>
              </ul>
            </dd>
          </div>
        )}
      </For>
    </dl>
  );
}

const sections: readonly TextSection[] = [
  {
    id: 'github-contributors',
    title: 'GitHub Contributors',
    level: 2,
    content: () => <People people={credits.contributors} />,
  },
  {
    id: 'testing-feedback',
    title: 'Testing & Feedback',
    level: 2,
    content: () => <People people={credits.testers} />,
  },
  {
    id: 'recipe-authors',
    title: 'Recipe Authors',
    level: 2,
    content: () => (
      <dl class="credits-recipes">
        <For each={credits.recipeAuthors}>
          {(author) => (
            <div>
              <dt>{author.name}</dt>
              <dd>
                <ul>
                  <For each={author.recipes}>
                    {(recipe) => (
                      <li>
                        <a href={recipe.source}>{recipe.name}</a>
                      </li>
                    )}
                  </For>
                </ul>
              </dd>
            </div>
          )}
        </For>
      </dl>
    ),
  },
  {
    id: 'photographs',
    title: 'Photographs',
    level: 2,
    content: () => <Photographs />,
  },
  {
    id: 'open-source',
    title: 'Open Source',
    level: 2,
    content: () => <OpenSource />,
  },
];

export default function Credits() {
  return <TextPage title="Credits" class="credits-page" sections={sections} />;
}
