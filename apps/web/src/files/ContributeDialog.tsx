import { Show, createEffect, createSignal } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { FileList, type FileListItem } from '@fujimod/emulator-ui/ui/file-list';
import './files.css';

export interface ContributeReceipt {
  id: string;
  link?: { href: string; label: string };
}

/** Uploads with `message`, reporting completion from 0 to 1. Rejects with a message to show. */
export type Contribute = (
  message: string,
  progress: (fraction: number) => void,
) => Promise<ContributeReceipt>;

type Phase =
  | { kind: 'idle' }
  | { kind: 'uploading'; fraction: number }
  | { kind: 'done'; receipt: ContributeReceipt }
  | { kind: 'failed'; error: string };

/** The Contribute dialog: disclosure, the files, a message, Cancel and Upload. */
export function ContributeDialog(props: {
  open: boolean;
  items: readonly FileListItem[];
  upload: Contribute;
  onClose: () => void;
  /** Extra choices or notices shown above the message, such as the bug report's public notice. */
  children?: JSX.Element;
}) {
  let dialog!: HTMLDialogElement;
  const [message, setMessage] = createSignal('');
  const [phase, setPhase] = createSignal<Phase>({ kind: 'idle' });
  createEffect(
    () => props.open,
    (open) => {
      if (!dialog) return;
      if (open && !dialog.open) {
        setMessage('');
        setPhase({ kind: 'idle' });
        dialog.showModal();
      } else if (!open && dialog.open) dialog.close();
    },
  );
  const busy = () => phase().kind === 'uploading';
  async function upload() {
    if (busy()) return;
    setPhase({ kind: 'uploading', fraction: 0 });
    try {
      const receipt = await props.upload(message().trim(), (fraction) =>
        setPhase({ kind: 'uploading', fraction }),
      );
      setPhase({ kind: 'done', receipt });
    } catch (error) {
      setPhase({ kind: 'failed', error: error instanceof Error ? error.message : String(error) });
    }
  }
  return (
    <dialog
      ref={dialog}
      class="contribute"
      aria-label="Contribute"
      onCancel={(event) => {
        event.preventDefault();
        if (!busy()) props.onClose();
      }}
      onClick={(event) => {
        if (event.target === dialog && !busy()) props.onClose();
      }}
    >
      <p>
        Sharing this data with FujiMod contributors can help us improve support for your camera. It
        may contain your camera serial number, wifi pairing code, current screen contents, or live
        sensor data. We believe this isn’t a security concern but you should be informed.
      </p>
      <FileList items={props.items} />
      {props.children}
      <label class="cd-field">
        Message (optional)
        <textarea
          rows="3"
          value={message()}
          disabled={busy() || phase().kind === 'done'}
          onInput={(event) => setMessage(event.currentTarget.value)}
        />
      </label>
      <div class="cd-foot">
        <div
          class={{
            'cd-status': true,
            ok: phase().kind === 'done',
            error: phase().kind === 'failed',
          }}
          role="status"
          aria-live="polite"
        >
          <Show when={phase().kind === 'uploading'}>
            <div class="cd-progress">
              <i
                style={{
                  width: `${Math.round(100 * (phase() as { fraction: number }).fraction)}%`,
                }}
              />
            </div>
            <span>
              Compressing and uploading ·{' '}
              {Math.round(100 * (phase() as { fraction: number }).fraction)}%
            </span>
          </Show>
          <Show
            when={phase().kind === 'done' && (phase() as { receipt: ContributeReceipt }).receipt}
          >
            {(receipt) => (
              <span class="cd-clip">
                Uploaded · <code>{receipt().id.slice(0, 8)}</code>
                <Show when={receipt().link}>
                  {(link) => (
                    <>
                      {' · '}
                      <a href={link().href} target="_blank" rel="noopener noreferrer">
                        {link().label}
                      </a>
                    </>
                  )}
                </Show>
              </span>
            )}
          </Show>
          <Show when={phase().kind === 'failed'}>
            <span class="cd-clip">{(phase() as { error: string }).error}</span>
          </Show>
        </div>
        <div class="cd-buttons">
          <Show
            when={phase().kind !== 'done'}
            fallback={
              <button type="button" class="cd-btn" onClick={() => props.onClose()}>
                Close
              </button>
            }
          >
            <button type="button" class="cd-btn" disabled={busy()} onClick={() => props.onClose()}>
              Cancel
            </button>
            <button
              type="button"
              class="cd-btn primary"
              disabled={busy()}
              onClick={() => void upload()}
            >
              Upload
            </button>
          </Show>
        </div>
      </div>
    </dialog>
  );
}
