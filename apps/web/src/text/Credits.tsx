import { For, Show } from 'solid-js';
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
    id: 'open-source',
    title: 'Open Source',
    level: 2,
    content: () => <OpenSource />,
  },
];

export default function Credits() {
  return <TextPage title="Credits" class="credits-page" sections={sections} />;
}
