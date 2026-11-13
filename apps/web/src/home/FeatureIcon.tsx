import featureIcons from '../ui/assets/feature-icons.svg?url';
import iconMetrics from '../ui/assets/feature-icon-metrics.generated.json';

type IconMetric = { box: number[]; view: number[]; width: number; height: number };

/** Native menu artwork, cropped to its ink and optically sized. */
export function FeatureIcon(props: { name: string }) {
  const metric = () => (iconMetrics as Record<string, IconMetric>)[props.name];
  return (
    <svg
      class="catalog-icon"
      aria-hidden="true"
      viewBox={metric().box.join(' ')}
      style={{ '--w': metric().width, '--h': metric().height }}
    >
      <use
        href={`${featureIcons}#${props.name}`}
        x={metric().view[0]}
        y={metric().view[1]}
        width={metric().view[2]}
        height={metric().view[3]}
      />
    </svg>
  );
}
