import { Show, createSignal } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { ManualFeatureLink as Link } from '../ui/ManualFeatureLink';
import { featureMedia, type Media } from './feature-media';

const recording = featureMedia();
const mediaRoot = '/artifacts/docs/';
const clips = (cards: string[]) => [
  ...new Map(
    cards.flatMap((card) => {
      const media = recording(card);
      return media ? [[media.video, media] as const] : [];
    }),
  ).values(),
];

function ReelVideo(props: { title: string; clips: readonly Media[] }) {
  const [frame, setFrame] = createSignal(0);
  const [failed, setFailed] = createSignal<ReadonlySet<string>>(new Set());
  const clip = () => {
    const available = props.clips.filter(({ video }) => !failed().has(video));
    return available[frame() % available.length];
  };
  return (
    <Show when={clip()} keyed>
      {(media) => (
        <video
          aria-label={props.title + ' walkthrough reel'}
          src={mediaRoot + media.video}
          poster={mediaRoot + media.still}
          muted
          playsinline
          autoplay
          preload="metadata"
          loop={props.clips.length - failed().size === 1}
          onEnded={() => setFrame((index) => index + 1)}
          onError={() => setFailed((files) => new Set([...files, media.video]))}
        />
      )}
    </Show>
  );
}

function Reel(props: { title: string; clips: readonly Media[]; children: JSX.Element }) {
  return (
    <figure class="home-reel">
      <ReelVideo title={props.title} clips={props.clips} />
      <figcaption>
        <h2>{props.title}</h2>
        <p>{props.children}</p>
      </figcaption>
    </figure>
  );
}

const shooting = clips(['shape_aspect_item', 'photometry', 'digital_tele_conv', 'snap_menu_item']);
const looks = clips([
  'film_simulation',
  'image_quality',
  'noise_reduction',
  'edit_save_custom_setting',
]);
const controls = clips([
  'bindings_row_exposure_compensation_dial',
  'menu_style_item',
  'menu_theme_item',
]);

export function FeatureReels(props: { recipeCount: number }) {
  return (
    <section class="home-reels" aria-label="Features">
      <Reel title="Shooting tools" clips={shooting}>
        Frame in{' '}
        <Link page="shape-aspect" row="ASPECT RATIO">
          ASPECT RATIO
        </Link>
        s from 4:3 to 65:24, meter for the brightest part of the scene with{' '}
        <Link page="metering" row="PHOTOMETRY" choice="HIGHLIGHT">
          HIGHLIGHT METERING
        </Link>
        , crop in with the{' '}
        <Link page="the-digital-teleconverter" row="DIGITAL TELE-CONV.">
          RAW+JPEG TELE-CONVERTER
        </Link>
        , and preset a distance with{' '}
        <Link page="focus" row="MF SNAP FOCUS">
          SNAP FOCUS
        </Link>
        .{' '}
        <Link page="focus" row="AUTOFOCUS">
          AUTOFOCUS
        </Link>{' '}
        is faster too.
      </Reel>
      <Reel title="Image looks + recipes" clips={looks}>
        New{' '}
        <Link page="film-simulation" row="FILM SIMULATION">
          FILM SIMULATION
        </Link>
        s and effects, every{' '}
        <Link page="image-size-and-image-quality" row="IMAGE QUALITY">
          IMAGE QUALITY SETTING
        </Link>{' '}
        down to switching{' '}
        <Link page="noise-reduction" row="NOISE REDUCTION">
          NOISE REDUCTION
        </Link>{' '}
        off, <Show when={props.recipeCount}>{props.recipeCount} </Show>
        <Link page="edit-save-custom-setting" row="EDIT/SAVE CUSTOM SETTING">
          RECIPES
        </Link>{' '}
        by name, and a one-press{' '}
        <Link page="disposable" row="DISPOSABLE">
          DISPOSABLE CAMERA
        </Link>{' '}
        mode.
      </Reel>
      <Reel title="Controls + system" clips={controls}>
        Put ISO on the{' '}
        <Link page="button-dial-setting" row="EV DIAL">
          EXPOSURE COMPENSATION DIAL
        </Link>
        , find anything in the new{' '}
        <Link page="menu-style" row="MENU STYLE">
          MENU
        </Link>
        ,{' '}
        <Link page="back-up-your-camera" row="BACK UP">
          BACK UP
        </Link>{' '}
        your settings to the SD card,{' '}
        <Link page="boot-setup" row="AUTOMATIC">
          AUTOSTART
        </Link>{' '}
        Fujimod, and{' '}
        <Link page="check-for-updates" row="CHECK FOR UPDATES">
          CHECK FOR UPDATES
        </Link>{' '}
        over Wi-Fi.
      </Reel>
    </section>
  );
}
