import { For, Show } from 'solid-js';
import { featureMedia } from '../home/feature-media';
import { releaseData, type SiteReleaseData } from '../release-data';
import './release-personal-note.css';

const dateFormat = new Intl.DateTimeFormat('en-US', { dateStyle: 'long', timeZone: 'UTC' });
const credits = '/credits';
const manual = '/docs/manual/';

export function ReleasePersonalNote(props: { data?: SiteReleaseData }) {
  const data = () => props.data ?? releaseData;
  const note = () => data().personalNote;
  const halation = featureMedia()('halation_strength_item', 'halation_color_item');
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
        Today we’re releasing the second version of Fujimod, a firmware modification designed to
        unlock the full potential of your Fujifilm camera.
      </p>
      <p>
        This release is an early preview of how we’re going to make Fujimod the best camera firmware
        in the world: packed with creative tools, configured with a slick menu interface on top of
        files you can back up and share, fully documented in our <a href={manual}>manual</a>, and
        engineered for <a href="/safety">rapid AI development while keeping your camera safe</a>.
      </p>
      <p>
        The first version of Fujimod I built alone. In the {note().daysSinceFirstRelease} days since
        then, <b>I</b> became <b>we</b> as {note().contributorCount} contributors joined the{' '}
        <a href="https://discord.gg/jWvFPakw7X">Fujihack.org Discord</a> to add features, test
        changes, and bring Fujimod to new cameras. Thank you{' '}
        <span class="note-hover">
          <a
            href={credits}
            onClick={(event) => {
              if (event instanceof PointerEvent && event.pointerType === 'touch') {
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
        I want to give a big shout out to <a href="https://github.com/heytchap">heytchap</a> for
        turning the X70 into a mini-X100VI-plus by building film simulations like{' '}
        <a class="note-feature" href={manual + 'film-effects/'}>
          COLOR NEGATIVE
        </a>{' '}
        and effects like{' '}
        <a class="note-feature" href={manual + 'grain-effect-and-clarity/'}>
          GRAIN
        </a>
        ,{' '}
        <a class="note-feature" href={manual + 'color/'}>
          COLOR CHROME FX
        </a>
        ,{' '}
        <a class="note-feature" href={manual + 'grain-effect-and-clarity/'}>
          CLARITY
        </a>{' '}
        and even{' '}
        <span class="note-hover">
          <a class="note-feature" href={manual + 'film-effects/'}>
            HALATION
          </a>
          <span class="note-popover">
            <Show when={halation}>
              {(media) => (
                <video src={`/artifacts/docs/${media().video}`} muted autoplay loop playsinline />
              )}
            </Show>
            <b>HALATION</b>
            <span>Adds the red glow film shows around bright highlights.</span>
            <em>IMAGE QUALITY SETTING › FILM SIMULATION</em>
          </span>
        </span>
        ; <a href="https://github.com/shanecranor">shane</a> +{' '}
        <a href="https://github.com/chris-rudmin">chris-rudmin</a> for adding our 2nd and 3rd bodies
        X-E2 and X-E2S and optimizing performance;{' '}
        <a href="https://www.instagram.com/jody_hinterleitner">jody</a> and anonymous for giving
        early valuable testing feedback and inspiring us to go farther.
      </p>
      <p>
        In the coming weeks we’ll be focused on maturing the Fujimod platform with support for more
        bodies and automated nightly releases (delivered by our{' '}
        <a class="note-feature" href={manual + 'check-for-updates/'}>
          Wi-Fi update system
        </a>
        ).
      </p>
      <p>
        Run on your camera without modifying your firmware via{' '}
        <a href="/#download">SD card dual-boot</a>, or{' '}
        <a href="/#software">try it first in your web browser</a> via our emulator.{' '}
        <a href="https://discord.gg/jWvFPakw7X">Join us on Discord</a> to report bugs, request
        features, or get involved with development.
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
