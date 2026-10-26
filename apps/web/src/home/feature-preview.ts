/** How long scrolling must rest on a tile before its preview starts on touch devices. */
const SETTLE_MS = 250;

type Preview = {
  activate: (active: boolean) => void;
  ratio: number;
  hovered: boolean;
  focused: boolean;
  available: boolean;
};

/** One observer and one playback slot shared by the mounted feature tiles. */
class FeaturePreviews {
  private entries = new Map<HTMLElement, Preview>();
  private active?: Preview;
  private pending?: ReturnType<typeof setTimeout>;
  private mobile = matchMedia('(hover: none), (pointer: coarse)');
  private observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        const preview = this.entries.get(entry.target as HTMLElement);
        if (preview) preview.ratio = entry.isIntersecting ? entry.intersectionRatio : 0;
      }
      this.update();
    },
    { threshold: [0, 0.25, 0.5, 0.75, 1] },
  );

  constructor() {
    this.mobile.addEventListener('change', this.update);
    document.addEventListener('visibilitychange', this.update);
  }

  private update = () => {
    let next: Preview | undefined;
    let priority = 0;
    if (!document.hidden) {
      for (const preview of this.entries.values()) {
        if (!preview.available) continue;
        const eligible = this.mobile.matches
          ? preview.ratio >= 0.5
          : preview.ratio > 0 && (preview.hovered || preview.focused);
        const score = this.mobile.matches ? preview.ratio : preview.hovered ? 2 : 1;
        if (eligible && (score > priority || (score === priority && preview === this.active))) {
          next = preview;
          priority = score;
        }
      }
    }
    // On touch devices the playing tile keeps playing while it stays mostly in
    // view, and a new one starts only once scrolling settles on it.
    if (this.mobile.matches && next && this.active?.available && this.active.ratio >= 0.75)
      next = this.active;
    clearTimeout(this.pending);
    if (next === this.active) return;
    const swap = () => {
      this.active?.activate(false);
      this.active = next;
      next?.activate(true);
    };
    if (this.mobile.matches && next) this.pending = setTimeout(swap, SETTLE_MS);
    else swap();
  };

  watch(element: HTMLElement, activate: Preview['activate']) {
    const preview: Preview = {
      activate,
      ratio: 0,
      hovered: false,
      focused: false,
      available: true,
    };
    this.entries.set(element, preview);
    this.observer.observe(element);
    return {
      hover: (hovered: boolean) => {
        preview.hovered = hovered;
        this.update();
      },
      focus: (focused: boolean) => {
        preview.focused = focused;
        this.update();
      },
      disable: () => {
        preview.available = false;
        this.update();
      },
      dispose: () => {
        this.observer.unobserve(element);
        this.entries.delete(element);
        this.update();
        if (!this.entries.size) {
          clearTimeout(this.pending);
          this.observer.disconnect();
          this.mobile.removeEventListener('change', this.update);
          document.removeEventListener('visibilitychange', this.update);
          previews = undefined;
        }
      },
    };
  }
}

let previews: FeaturePreviews | undefined;
export function watchFeaturePreview(element: HTMLElement, activate: Preview['activate']) {
  previews ??= new FeaturePreviews();
  return previews.watch(element, activate);
}
