import { createSignal, onSettled } from 'solid-js';

export function projectAge(date: Date) {
  const today = Date.UTC(date.getFullYear(), date.getMonth(), date.getDate());
  return Math.max(0, Math.floor((today - Date.UTC(2026, 9, 3)) / 86_400_000));
}

declare const __FUJIMOD_BUILD_DAY__: string;
export function PersonalNote() {
  const [age, setAge] = createSignal(projectAge(new Date(`${__FUJIMOD_BUILD_DAY__}T12:00:00`)));
  onSettled(() => {
    setAge(projectAge(new Date()));
  });
  return (
    <aside class="personal-note" aria-label="A note from Jake">
      <p>Hey,</p>
      <p>
        FujiMod is not ready for your camera. It is <span id="project-age">{age()}</span> days old
        and changing too quickly. I’ll release SD card dual boot and then installable firmware as I
        gain confidence in our simulation testing.
      </p>
      <p>
        Until then, you can try it in the <a href="#workspace">camera emulator</a> below.
      </p>
      <p class="signature">
        Yours truly,
        <br />
        <span class="signature-name">Jake</span>
      </p>
    </aside>
  );
}
