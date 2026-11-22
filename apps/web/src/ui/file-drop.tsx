import { createSignal } from 'solid-js';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import './file-drop.css';

/** A dashed drop target with a "choose a file" link for one or more files. */
interface FileDropOptions {
  inputId: string;
  title: string;
  /** Text before the "choose a file" link, such as "Drop a Fujifilm .DAT file here, or". */
  prompt: string;
  accept?: string;
  /** Accessible name of the hidden file input. */
  label: string;
  /** Tints the target blue when it is the next thing to do. */
  highlighted?: boolean;
  disabled?: boolean;
}
export function FileDrop(
  props: FileDropOptions &
    (
      | { multiple?: false; onFile(file: File | undefined): void }
      | { multiple: true; onFiles(files: File[]): void }
    ),
) {
  let input: HTMLInputElement | undefined;
  function choose(files: FileList | null | undefined) {
    if (props.disabled) return;
    if (props.multiple) props.onFiles(Array.from(files ?? []));
    else props.onFile(files?.[0]);
  }
  const [dragging, setDragging] = createSignal(false);
  return (
    <div
      class={{
        'file-drop': true,
        'file-drop-highlighted': !!props.highlighted,
        dragging: dragging(),
      }}
      onDragOver={(event) => {
        event.preventDefault();
        event.stopPropagation();
        setDragging(true);
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={(event) => {
        event.preventDefault();
        event.stopPropagation();
        setDragging(false);
        choose(event.dataTransfer?.files);
      }}
    >
      <span class="file-drop-icon">
        <Icon name="plus" size={20} />
      </span>
      <strong>{props.title}</strong>
      <span>
        {props.prompt}{' '}
        <button
          class="file-drop-choose"
          type="button"
          disabled={props.disabled}
          onClick={() => input?.click()}
        >
          {props.multiple ? 'choose files' : 'choose a file'}
        </button>
      </span>
      <input
        ref={(element) => {
          input = element;
        }}
        class="visually-hidden"
        tabindex={-1}
        id={props.inputId}
        type="file"
        accept={props.accept}
        multiple={props.multiple}
        aria-label={props.label}
        disabled={props.disabled}
        onChange={(event) => {
          choose(event.currentTarget.files);
          event.currentTarget.value = '';
        }}
      />
    </div>
  );
}
