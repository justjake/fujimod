import { For, createSignal, onSettled } from 'solid-js';

/** The card body behind the download section, drawn at the section's measured size. */
export function SdCardShape() {
  const [size, setSize] = createSignal({ width: 820, height: 1200 });
  let svg!: SVGSVGElement;
  onSettled(() => {
    const parent = svg.parentElement!;
    const observer = new ResizeObserver(() =>
      setSize({ width: parent.clientWidth, height: parent.clientHeight }),
    );
    observer.observe(parent);
    return () => observer.disconnect();
  });
  const geometry = () => {
    const { width: w, height: h } = size();
    const chamfer = Math.min(Math.round(w * 0.14), 112),
      radius = 22,
      joint = 7,
      // The write-protect notch on the left edge, with its slider.
      notchTop = 150,
      notchBottom = 270,
      notchDepth = 9;
    const d = [
      `M ${radius} 0`,
      `H ${w - chamfer - joint}`,
      `Q ${w - chamfer} 0 ${w - chamfer + joint * 0.7} ${joint * 0.7}`,
      `L ${w - joint * 0.7} ${chamfer - joint * 0.7}`,
      `Q ${w} ${chamfer} ${w} ${chamfer + joint}`,
      `V ${h - radius}`,
      `A ${radius} ${radius} 0 0 1 ${w - radius} ${h}`,
      `H ${radius}`,
      `A ${radius} ${radius} 0 0 1 0 ${h - radius}`,
      `V ${notchBottom}`,
      `H ${notchDepth}`,
      `V ${notchTop}`,
      `H 0`,
      `V ${radius}`,
      `A ${radius} ${radius} 0 0 1 ${radius} 0`,
      'Z',
    ].join(' ');
    // Grip ridges molded across the bottom edge.
    const ridges = Array.from({ length: 6 }, (_, i) => h - 24 - i * 7);
    return { w, h, d, notchTop, notchBottom, notchDepth, ridges };
  };
  return (
    <svg
      ref={(element) => {
        svg = element;
      }}
      class="sd-shape"
      aria-hidden="true"
      viewBox={`0 0 ${geometry().w} ${geometry().h}`}
      width={geometry().w}
      height={geometry().h}
    >
      <defs>
        <linearGradient id="sd-body" x1="0" y1="0" x2="0.35" y2="1">
          <stop offset="0" stop-color="#2a2a2d" />
          <stop offset="0.35" stop-color="#1c1c1f" />
          <stop offset="1" stop-color="#141416" />
        </linearGradient>
        <linearGradient id="sd-edge" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="rgb(255 255 255 / 28%)" />
          <stop offset="0.25" stop-color="rgb(255 255 255 / 10%)" />
          <stop offset="1" stop-color="rgb(255 255 255 / 6%)" />
        </linearGradient>
        <linearGradient id="sd-slider" x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" stop-color="#d1d1d6" />
          <stop offset="1" stop-color="#8e8e93" />
        </linearGradient>
      </defs>
      <path d={geometry().d} fill="url(#sd-body)" stroke="url(#sd-edge)" stroke-width="1.5" />
      <rect
        x="1"
        y={geometry().notchTop + 10}
        width={geometry().notchDepth + 3}
        height="44"
        rx="2.5"
        fill="url(#sd-slider)"
      />
      <For each={geometry().ridges}>
        {(y) => (
          <line
            x1="40"
            x2={geometry().w - 40}
            y1={y}
            y2={y}
            stroke="rgb(255 255 255 / 5%)"
            stroke-width="2"
          />
        )}
      </For>
    </svg>
  );
}
