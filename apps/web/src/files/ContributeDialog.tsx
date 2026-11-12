import { Show, createEffect, createSignal } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { FileList, type FileListItem } from '@fujimod/emulator-ui/ui/file-list';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import './files.css';

export interface ContributeReceipt {
  id: string;
  shareUrl: string;
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
  /** Offered after a successful upload, such as contributing the next report. */
  next?: { label: string; run: () => void };
}) {
  let dialog!: HTMLDialogElement;
  const [message, setMessage] = createSignal('');
  const [phase, setPhase] = createSignal<Phase>({ kind: 'idle' });
  const [copied, setCopied] = createSignal(false);
  createEffect(
    () => props.open,
    (open) => {
      if (!dialog) return;
      if (open && !dialog.open) {
        setMessage('');
        setPhase({ kind: 'idle' });
        setCopied(false);
        dialog.showModal();
      } else if (!open && dialog.open) dialog.close();
    },
  );
  const busy = () => phase().kind === 'uploading';
  async function copyShareUrl(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  }
  async function upload() {
    if (busy()) return;
    setPhase({ kind: 'uploading', fraction: 0 });
    try {
      const receipt = await props.upload(message().trim(), (fraction) =>
        setPhase({ kind: 'uploading', fraction }),
      );
      setPhase({ kind: 'done', receipt });
      await copyShareUrl(receipt.shareUrl);
    } catch (error) {
      setPhase({ kind: 'failed', error: shownError(error) });
    }
  }
  return (
    <dialog
      ref={(element) => (dialog = element)}
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
        sensor data. We believe this isn’t a security concern but you should be informed. Anyone
        with the share URL can download these files.
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
                {copied() ? 'Share URL copied' : 'Uploaded'}
                {' · '}
                <a href={receipt().shareUrl} target="_blank" rel="noopener noreferrer">
                  Share URL
                </a>
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
              <>
                <Show when={!copied()}>
                  <button
                    type="button"
                    class="cd-btn"
                    onClick={() => {
                      const current = phase();
                      if (current.kind === 'done') void copyShareUrl(current.receipt.shareUrl);
                    }}
                  >
                    Copy URL
                  </button>
                </Show>
                <button type="button" class="cd-btn" onClick={() => props.onClose()}>
                  Close
                </button>
                <Show when={props.next}>
                  {(next) => (
                    <button type="button" class="cd-btn primary" onClick={() => next().run()}>
                      {next().label}
                    </button>
                  )}
                </Show>
              </>
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
