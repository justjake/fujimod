import { createEffect, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';
import './install-dialog.css';

/** The modal that holds a FujiMod download and its install steps. */
export function InstallDialog(props: {
  title: string;
  identity: string;
  busy: boolean;
  close: () => void;
  children: JSX.Element;
}) {
  let dialog!: HTMLDialogElement;
  onSettled(() => {
    dialog.showModal();
    return () => {
      if (dialog.open) dialog.close();
    };
  });
  createEffect(
    () => props.busy,
    (busy) => {
      dialog.setAttribute('aria-busy', String(busy));
    },
  );
  return (
    <dialog
      ref={(element) => {
        dialog = element;
      }}
      class="install-dialog"
      aria-labelledby="install-dialog-title"
      onClose={() => props.close()}
    >
      <h2 id="install-dialog-title">{props.title}</h2>
      <p class="install-dialog-identity">{props.identity}</p>
      {props.children}
      <form method="dialog" class="install-dialog-close">
        <button class="pill">Close</button>
      </form>
    </dialog>
  );
}
