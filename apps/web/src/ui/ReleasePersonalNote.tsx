import { For, Show } from 'solid-js';
import { ManualFeatureLink } from './ManualFeatureLink';
import { releaseData, type SiteReleaseData } from '../release-data';
import './release-personal-note.css';

const dateFormat = new Intl.DateTimeFormat('en-US', { dateStyle: 'long', timeZone: 'UTC' });
const credits = '/credits';
const manual = '/docs/manual/';

export function ReleasePersonalNote(props: { data?: SiteReleaseData }) {
  const data = () => props.data ?? releaseData;
  const note = () => data().personalNote;
  return (
    <article class="release-personal-note" aria-label="A note from Jake">
      <Show when={data().current}>
        {(release) => (
          <time datetime={release().publishedAt}>
            {dateFormat.format(new Date(release().publishedAt))}
          </time>
        )}
      </Show>
      <p>
        Today we’re releasing the second version of Fujimod, a firmware modification that unlocks
        the full potential of your Fujifilm camera.
      </p>
      <p>
        It’s an early look at how we’ll make Fujimod the best camera firmware in the world: packed with creative tools, configured with a slick menu interface on top of
        files you can back up and share, fully documented in our <a href={manual}>manual</a>, and
        engineered for <a href="/safety">rapid AI development while keeping your camera safe</a>.
      </p>
      <p>
        I built the first version alone. In the {note().daysSinceFirstRelease} days since,{' '}
        <b>I</b> became <b>we</b>: {note().contributorCount} contributors joined the{' '}
        <a href="https://discord.gg/jWvFPakw7X">Fujihack.org Discord</a> to add features, test
        changes and bring Fujimod to new cameras. Thank you,{' '}
        <span class="note-hover">
          <a
            href={credits}
            onClick={(event) => {
              const pointerType = event instanceof PointerEvent ? event.pointerType : '';
              const touchClick =
                pointerType === 'touch' ||
                (event.detail > 0 && matchMedia('(hover: none)').matches);
              if (touchClick) {
                event.preventDefault();
                event.currentTarget.focus();
              }
            }}
          >
            everyone!
          </a>
          <span class="note-popover note-people">
            <b>Contributors</b>
            <span class="note-names" role="region" aria-label="Contributors" tabindex={0}>
              <For each={note().contributorNames}>{(name) => <span>{name}</span>}</For>
            </span>
            <a class="note-credits" href={credits}>
              All credits →
            </a>
          </span>
        </span>{' '}
        Special thanks to <a href="https://github.com/heytchap">heytchap</a> for turning the X70
        into a mini X100VI-plus with film simulations like{' '}
        <ManualFeatureLink page="film-effects" row="NEGATIVE">
          COLOR NEGATIVE
        </ManualFeatureLink>{' '}
        and effects like{' '}
        <ManualFeatureLink
          page="grain-effect-and-clarity"
          row="GRAIN EFFECT"
          card="film_grain_graininess_item"
        >
          GRAIN
        </ManualFeatureLink>
        ,{' '}
        <ManualFeatureLink page="color" row="COLOR CHROME EFFECT">
          COLOR CHROME FX
        </ManualFeatureLink>
        ,{' '}
        <ManualFeatureLink page="grain-effect-and-clarity" row="CLARITY">
          CLARITY
        </ManualFeatureLink>{' '}
        and even{' '}
        <ManualFeatureLink page="film-effects" row="HALATION" card="halation_strength_item">
          HALATION
        </ManualFeatureLink>
        ; <a href="https://github.com/shanecranor">shane</a> and{' '}
        <a href="https://github.com/chris-rudmin">chris-rudmin</a> for bringing up the X-E2 and
        X-E2S and speeding things up; and{' '}
        <a href="https://www.instagram.com/jody_hinterleitner">jody</a> and an anonymous tester for
        early feedback that pushed us further.
      </p>
      <p>
        Next we’re maturing the platform: more cameras, and nightly releases delivered by our{' '}
        <ManualFeatureLink page="check-for-updates" row="CHECK FOR UPDATES">
          Wi-Fi update system
        </ManualFeatureLink>
        .
      </p>
      <p>
        Run it from your <a href="/#download">SD card</a> without changing your camera’s firmware,
        or <a href="/#software">try it in your browser</a> first.{' '}
        <a href="https://discord.gg/jWvFPakw7X">Join us on Discord</a> to report bugs, request
        features or help build it.
      </p>
      <p class="note-signature">
        Yours truly,
        <br />
        Jake
        <span class="note-socials">
          <a href="https://discord.com/users/177295934152114178">Discord</a> ·{' '}
          <a href="https://twitter.com/jitl">Twitter</a> ·{' '}
          <a href="https://instagram.com/_jitl">Instagram</a> ·{' '}
          <a href="https://buymeacoffee.com/jitl">Buy me a coffee</a>
        </span>
      </p>
    </article>
  );
}
