import { For, Show } from 'solid-js';
import type { LoadedRelease } from '../patch/release';
import { ReleaseNotes } from '../cameras/ReleaseNotes';
import { FeatureIcon } from './FeatureIcon';
import './release-showcase.css';

const highlights = [
  {
    icon: 'theme',
    title: 'A fresh view of familiar controls.',
    text: 'Focalized UI brings a new menu, Q menu and native text keyboard to X70. X-E2 and X-E2S keep the stock-style menu, with new themes.',
    query: 'theme',
  },
  {
    icon: 'film',
    title: 'More films. Your own color.',
    text: 'ETERNA, ETERNA BLEACH BYPASS, NOSTALGIC Neg., REALA ACE and ACROS join the film simulation lineup. Fine-tune a film, or start with LEICA X.',
    query: 'film',
  },
  {
    icon: 'grain',
    title: 'Find a different texture.',
    text: 'Color Chrome, Clarity and Grain Effect. Halation, light leaks, borders and a date stamp. Shape the JPEG in your camera.',
    query: 'effect',
  },
  {
    icon: 'recipes',
    title: 'Keep your favorite looks close.',
    text: 'Save up to 255 custom settings on X70, with seven favorites in the Q menu. Name, organize and share your recipes as files on the SD card.',
    query: 'custom',
  },
  {
    icon: 'controls',
    title: 'Make the camera your own.',
    text: 'New aspect ratios and framing guides. Snap focus and zone focus. More control assignments, a clearer HUD and recipe names in EXIF.',
    query: 'focus',
  },
  {
    icon: 'card',
    title: 'Start with an SD card.',
    text: 'SD boot v2 for X70, X-E2S and X-E2. The initial boot leaves the installed firmware unchanged. X70 can download later updates over Wi-Fi.',
    query: 'boot',
  },
];

export function ReleaseShowcase(props: { release?: LoadedRelease }) {
  const current = () => props.release?.release === '0.1.0';
  return (
    <section class="release-showcase" aria-labelledby="release-title">
      <div class="release-intro">
        <p class="release-eyebrow">
          FUJIMOD / 0.1.0
          <Show when={!current() || props.release?.prerelease}>
            <span>Preview</span>
          </Show>
        </p>
        <h1 id="release-title">
          A familiar camera.
          <br />
          New possibilities.
        </h1>
        <p class="release-lede">
          New films, new textures, and more ways to make your camera yours. FujiMod brings a new
          chapter to three Fujifilm classics.
        </p>
        <nav class="release-links" aria-label="On this page">
          <a class="release-primary" href="#quickstart">
            Get started <span aria-hidden="true">↓</span>
          </a>
          <a href="#features">Explore all features</a>
          <a href="#software">Try in your browser</a>
        </nav>
        <ul class="release-bodies" aria-label="0.1.0 supported cameras">
          <li>
            <b>X70</b>
            <span>Firmware 1.10</span>
          </li>
          <li>
            <b>X-E2S</b>
            <span>Firmware 1.11</span>
          </li>
          <li>
            <b>X-E2</b>
            <span>Firmware 4.11</span>
          </li>
        </ul>
      </div>
      <div class="release-highlights">
        <For each={highlights}>
          {(highlight) => (
            <a class="release-highlight" href={`#features?search=${highlight.query}`}>
              <FeatureIcon name={highlight.icon} />
              <h2>{highlight.title}</h2>
              <p>{highlight.text}</p>
              <span class="release-explore">
                Explore <span aria-hidden="true">↗</span>
              </span>
            </a>
          )}
        </For>
      </div>
      <section class="release-quickstart" id="quickstart" aria-labelledby="quickstart-title">
        <h2 class="section-heading" id="quickstart-title">
          Start with your SD card
        </h2>
        <ol>
          <li>
            <b>Check your camera.</b>
            <span>Use the exact model and firmware version listed above.</span>
          </li>
          <li>
            <b>Prepare the SD card.</b>
            <span>
              Back up your photos. Use a FAT32 SD card, then choose your camera in the builder
              below.
            </span>
          </li>
          <li>
            <b>Copy and boot.</b>
            <span>
              Extract the download to the SD card root. Hold the buttons shown for your camera below
              while switching it on, then release them.
            </span>
          </li>
        </ol>
        <p class="release-safety">
          No firmware update file is needed. To return to stock, turn the camera off and remove the
          SD card. Features and controls vary by camera; the catalog below describes X70.
        </p>
        <Show when={props.release?.notesText}>
          {(notes) => (
            <details class="release-current-notes" id="release-notes">
              <summary>
                Release notes &amp; known issues{' '}
                <span>
                  {props.release?.tag}
                  {props.release?.prerelease ? ' preview' : ''}
                </span>
              </summary>
              <ReleaseNotes text={notes()} />
            </details>
          )}
        </Show>
      </section>
    </section>
  );
}
