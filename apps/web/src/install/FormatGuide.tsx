import { For, createSignal, onSettled } from 'solid-js';
import { SegmentedControl } from '@fujimod/emulator-ui/ui/SegmentedControl';
import { SYSTEMS, FORMAT_STEPS, detectSystem, type System } from './format-guide';
import './format-guide.css';

/** How to format an SD card as FAT32 on the visitor's desktop system, with a picker for the others. */
export function FormatGuide() {
  const [system, setSystem] = createSignal<System>('macOS');
  onSettled(() => {
    setSystem(detectSystem(navigator.userAgent));
  });
  return (
    <div class="format-guide">
      <SegmentedControl
        label="System"
        options={SYSTEMS.map((name) => ({ value: name, label: name }))}
        value={system()}
        onSelect={setSystem}
      />
      <ol>
        <For each={FORMAT_STEPS[system()]}>{(step) => <li>{step}</li>}</For>
      </ol>
    </div>
  );
}
