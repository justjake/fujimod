import { For, Show, createSignal, createEffect, onSettled } from 'solid-js';
import './firmware-actions.css';
export interface DownloadFlags {
  allowFpUpdateDownload: boolean;
  allowSdCardBootDownload: boolean;
}
export type FirmwareAction = 'launch' | 'fp-update' | 'sd-card';
export function firmwareActions(
  flags: DownloadFlags,
  available: { fpUpdate: boolean; sdCard: boolean },
): FirmwareAction[] {
  return [
    'launch',
    ...(flags.allowFpUpdateDownload && available.fpUpdate ? ['fp-update' as const] : []),
    ...(flags.allowSdCardBootDownload && available.sdCard ? ['sd-card' as const] : []),
  ];
}
export function createFirmwareChoice() {
  const [choice, setChoice] = createSignal<{ actions: FirmwareAction[]; label: string }>();
  let resolve: ((value?: FirmwareAction) => void) | undefined;
  function finish(value?: FirmwareAction) {
    resolve?.(value);
    resolve = undefined;
    setChoice(undefined);
  }
  return {
    choice,
    cancel: () => finish(),
    finish,
    choose(actions: FirmwareAction[], label: string): Promise<FirmwareAction | undefined> {
      finish();
      if (actions.length === 1) return Promise.resolve(actions[0]);
      setChoice({ actions, label });
      return new Promise((done) => {
        resolve = done;
      });
    },
  };
}
export function FirmwareChoice(props: { choice: ReturnType<typeof createFirmwareChoice> }) {
  let dialog!: HTMLDialogElement;
  createEffect(
    () => props.choice.choice(),
    (value) => {
      if (value && !dialog.open) {
        dialog.returnValue = '';
        dialog.showModal();
      } else if (!value && dialog.open) dialog.close();
    },
  );
  onSettled(() => () => props.choice.cancel());
  return (
    <dialog
      ref={dialog}
      class="firmware-actions"
      aria-label={props.choice.choice()?.label}
      onClose={() =>
        props.choice.finish(
          props.choice.choice()?.actions.find((action) => action === dialog.returnValue),
        )
      }
    >
      <form method="dialog">
        <h3>{props.choice.choice()?.label}</h3>
        <div class="act">
          <For each={[...(props.choice.choice()?.actions ?? []), 'cancel']}>
            {(action) => (
              <button class="pill" value={action}>
                {
                  {
                    launch: 'Launch camera',
                    'fp-update': 'Download FP update',
                    'sd-card': 'Download SD card files',
                    cancel: 'Cancel',
                  }[action]
                }
              </button>
            )}
          </For>
        </div>
      </form>
    </dialog>
  );
}
