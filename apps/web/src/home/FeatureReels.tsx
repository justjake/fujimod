import { Show, createSignal } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { ManualFeatureLink as Link } from '../ui/ManualFeatureLink';
import { reelMedia, type ReelMedia } from './feature-media';

const mediaRoot = '/artifacts/docs/';

function ReelVideo(props: { title: string; media: ReelMedia }) {
  const [time, setTime] = createSignal(0);
  const caption = () =>
    props.media.cues.find(({ start, end }) => start <= time() && time() < end)?.text;
  const syncTime: JSX.EventHandler<HTMLVideoElement, Event> = (event) =>
    setTime(event.currentTarget.currentTime);

  return (
    <div class="home-reel-video">
      <video
        aria-label={props.title + ' walkthrough reel'}
        src={mediaRoot + props.media.video}
        poster={mediaRoot + props.media.still}
        muted
        playsinline
        autoplay
        loop
        preload="metadata"
        onTimeUpdate={syncTime}
        onSeeked={syncTime}
      />
      <Show when={caption()}>
        <span class="home-reel-caption">{caption()}</span>
      </Show>
    </div>
  );
}

function Reel(props: { id: string; title: string; children: JSX.Element }) {
  return (
    <figure id={`reel-${props.id}`} class="home-reel">
      <Show when={reelMedia(props.id)} keyed>
        {(media) => <ReelVideo title={props.title} media={media} />}
      </Show>
      <figcaption>
        <h2>{props.title}</h2>
        <p>{props.children}</p>
      </figcaption>
    </figure>
  );
}

export function FeatureReels(props: { recipeCount: number }) {
  return (
    <section class="home-reels" aria-label="Features">
      <Reel id="shooting" title="Shooting tools">
        Frame a panorama with{' '}
        <Link page="shape-aspect" row="ASPECT RATIO">
          ASPECT RATIO
        </Link>
        , meter for the brightest part of the scene with{' '}
        <Link page="metering" row="PHOTOMETRY" choice="HIGHLIGHT">
          HIGHLIGHT METERING
        </Link>
        , and crop in with the{' '}
        <Link page="the-digital-teleconverter" row="DIGITAL TELE-CONV.">
          DIGITAL TELECONVERTER
        </Link>{' '}
        in{' '}
        <Link page="image-size-and-image-quality" row="IMAGE QUALITY" choice="FINE+RAW">
          FINE+RAW
        </Link>
        .
      </Reel>
      <Reel id="looks" title="Image looks + recipes">
        Pick from <Show when={props.recipeCount}>{props.recipeCount} </Show>
        <Link page="edit-save-custom-setting" row="LIBRARY">
          RECIPES
        </Link>{' '}
        by name and add a glow around highlights with{' '}
        <Link page="film-effects" row="HALATION">
          HALATION
        </Link>
        .
      </Reel>
      <Reel id="controls" title="Controls + system">
        Choose the Fujimod{' '}
        <Link page="menu-style" row="MENU STYLE">
          MENU STYLE
        </Link>
        , put ISO on the{' '}
        <Link page="button-dial-setting" row="EV DIAL">
          EV DIAL
        </Link>
        , and{' '}
        <Link page="check-for-updates" row="CHECK FOR UPDATES">
          CHECK FOR UPDATES
        </Link>{' '}
        over Wi-Fi.
      </Reel>
    </section>
  );
}
