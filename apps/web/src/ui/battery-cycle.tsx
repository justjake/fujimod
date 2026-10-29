import './battery-cycle.css';

/** Camera battery with separate removal and reinsertion arrows. */
export function BatteryCycleIcon() {
  return (
    <svg class="battery-cycle-icon" viewBox="0 0 40 36" aria-hidden="true">
      <rect x="13" y="7" width="14" height="25" rx="2" />
      <path d="M17 7V4h6v3M17 12h6M17 27h6" />
      <path d="M6 27V9m-4 4 4-4 4 4M34 9v18m-4-4 4 4 4-4" />
    </svg>
  );
}
