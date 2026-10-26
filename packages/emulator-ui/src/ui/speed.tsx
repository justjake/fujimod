import { Icon } from './icons';
function speedColor(speed: number) {
  if (speed > 1) {
    return '#30d158';
  } else if (speed >= 0.75) {
    return 'var(--dim)';
  } else if (speed <= 0.2) {
    return 'var(--red)';
  } else {
    const warmth = (0.75 - speed) / 0.55;
    const [from, to, amount]: [string, string, number] =
      warmth < 0.5
        ? ['var(--dim)', '#ff9f0a', warmth * 2]
        : ['#ff9f0a', 'var(--red)', (warmth - 0.5) * 2];
    return `color-mix(in srgb, ${from}, ${to} ${amount * 100}%)`;
  }
}
/** A gauge colored by emulation speed, with the percentage beside it. */
export function SpeedGauge(props: { speed: number }) {
  // Nearest 5% keeps the reading steady while menus and live view alternate; color follows the shown value.
  const shown = () => Math.round(Math.max(0, props.speed) * 20) / 20;
  return (
    <span
      id="camera-speed"
      title="Emulation speed"
      aria-label={`Emulation speed ${Math.round(shown() * 100)}%`}
      style={{
        display: 'inline-flex',
        'align-items': 'center',
        gap: '4px',
        'white-space': 'nowrap',
        'font-variant-numeric': 'tabular-nums',
        color: speedColor(shown()),
      }}
    >
      <Icon name="gauge" size={14} />
      {Math.round(shown() * 100)}%
    </span>
  );
}
