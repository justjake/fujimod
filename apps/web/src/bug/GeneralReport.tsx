import { createSignal, For, onSettled, Show } from 'solid-js';
import { formatBytes } from '@fujimod/emulator-ui/ui/file-list';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import { zipBlob } from '@fujimod/emulator-ui/archive';
import { uploadReportArchive } from '../files/contribute';
import { DesktopSystemGuide } from '../install/FormatGuide';
import { desktopSdCardLocations } from './file-guide';
import { sdCardFiles, type ReportFile } from './sd-card-files';
import { FileDrop } from '../ui/file-drop';
import { CONTACT_TYPES, MAX_REPORT_ARCHIVE_BYTES, type ContactType } from './general-report';
import { submitReport, type ReportReceipt } from './upload';

export function GeneralReport() {
  const [files, setFiles] = createSignal<ReportFile[]>([]);
  const [status, setStatus] = createSignal('');
  let manualIndex = 0;
  let archive: Blob | undefined;
  const [contact, setContact] = createSignal<ContactType>();
  const [busy, setBusy] = createSignal(false);
  const [ready, setReady] = createSignal(false);
  onSettled(() => {
    setReady(true);
  });
  const [sdError, setSdError] = createSignal('');
  const [error, setError] = createSignal('');
  const [receipt, setReceipt] = createSignal<ReportReceipt>();
  let id: string | undefined;

  function changeFiles(next: ReportFile[]) {
    id = undefined;
    archive = undefined;
    setError('');
    setFiles(next);
  }

  function addFiles(added: File[]) {
    changeFiles([
      ...files(),
      ...added.map((file) => ({
        file,
        path: `Attachments/${++manualIndex}/${file.name.replaceAll('\\', '_')}`,
        included: true,
      })),
    ]);
  }

  function readSdCard(event: Event & { currentTarget: HTMLInputElement }) {
    const input = event.currentTarget;
    if (!input.files?.length) return;
    const found = sdCardFiles(Array.from(input.files));
    input.value = '';
    if (!found.length) {
      setSdError('No FujiMod files found. Choose the SD card drive.');
      return;
    }
    setSdError('');
    const manual = files().filter((entry) => entry.path.startsWith('Attachments/'));
    changeFiles([...manual, ...found]);
  }

  function selectContact(event: Event & { currentTarget: HTMLSelectElement }) {
    const value = event.currentTarget.value;
    setContact(CONTACT_TYPES.find((type) => type === value));
  }

  async function submit(event: SubmitEvent & { currentTarget: HTMLFormElement }) {
    event.preventDefault();
    if (busy() || receipt()) return;
    const body = new FormData(event.currentTarget);
    const value = body.get('description');
    const description = typeof value === 'string' ? value.trim() : '';
    const selected = files().filter((entry) => entry.included);
    if (!description && !selected.length) {
      setError('Describe what happened or attach a file.');
      return;
    }
    body.set('kind', 'general');
    body.set('description', description);
    setError('');
    setBusy(true);
    try {
      const reportId = (id ??= crypto.randomUUID());
      if (selected.length) {
        if (!archive) {
          setStatus('Creating ZIP…');
          archive = await zipBlob(selected.map(({ path, file }) => ({ path, data: file })));
        }
        if (archive.size > MAX_REPORT_ARCHIVE_BYTES)
          throw Error('The ZIP exceeds 1 GiB. Uncheck some files and try again.');
        await uploadReportArchive(reportId, archive, ({ done, total }) => {
          setStatus(`Uploading ZIP · ${Math.round((done / total) * 100)}%`);
        });
        body.set('archive', 'zip');
      }
      setStatus('Submitting report…');
      setReceipt(await submitReport(body, reportId));
    } catch (cause) {
      setError(shownError(cause));
    } finally {
      setStatus('');
      setBusy(false);
    }
  }

  return (
    <section class="bug-general" aria-labelledby="general-bug-title">
      <h1 id="general-bug-title">Report a bug</h1>
      <Show
        when={receipt()}
        fallback={
          <form
            method="post"
            onSubmit={(event) => void submit(event)}
            onInput={() => {
              id = undefined;
            }}
          >
            <fieldset disabled={!ready() || busy()}>
              <div class="bug-report-description">
                <label for="bug-description">What happened?</label>
                <p id="bug-description-help" class="bug-status">
                  Be as detailed as possible. Include the exact actions you took and the buttons you
                  pressed. Explain how to trigger the bug as if you were writing for someone who’s
                  never used a camera before.
                </p>
                <textarea
                  id="bug-description"
                  name="description"
                  rows="5"
                  maxlength={12000}
                  aria-describedby="bug-description-help"
                />
              </div>
              <FileDrop
                inputId="bug-attachments"
                title="Attachments"
                prompt="Drop files here, or"
                label="Choose attachments"
                multiple
                disabled={busy()}
                onFiles={addFiles}
              />
              <div>
                <label>
                  SD card diagnostics
                  <input
                    type="file"
                    webkitdirectory
                    multiple
                    aria-describedby="bug-sd-help"
                    onChange={readSdCard}
                  />
                </label>
                <p id="bug-sd-help" class="bug-status">
                  Choose the SD card drive to attach FujiMod files, boot diagnostics, and backups.
                </p>
                <DesktopSystemGuide
                  instructions={(system) => (
                    <p class="bug-status">{desktopSdCardLocations[system]} → your SD card</p>
                  )}
                />
                <Show when={sdError()}>
                  <p role="alert">{sdError()}</p>
                </Show>
              </div>
              <Show when={files().length}>
                <button
                  class="pill"
                  type="button"
                  onClick={() => {
                    const include = !files().some((file) => file.included);
                    changeFiles(files().map((file) => ({ ...file, included: include })));
                  }}
                >
                  {files().some((file) => file.included) ? 'Uncheck all' : 'Check all'}
                </button>
                <ul class="bug-attachments" aria-label="Files to include">
                  <For each={files()} keyed={false}>
                    {(entry, index) => (
                      <li>
                        <label>
                          <input
                            type="checkbox"
                            checked={entry().included}
                            onChange={(event) => {
                              const included = event.currentTarget.checked;
                              changeFiles(
                                files().map((file, position) =>
                                  position === index ? { ...file, included } : file,
                                ),
                              );
                            }}
                          />
                          <span>
                            {entry().path.startsWith('Attachments/')
                              ? entry().file.name
                              : entry().path}{' '}
                            ({formatBytes(entry().file.size)})
                          </span>
                        </label>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
              <label>
                Name (optional)
                <input name="name" autocomplete="name" maxlength={120} />
              </label>
              <label>
                Contact for updates (optional)
                <select name="contact-type" onChange={selectContact}>
                  <option value="">None</option>
                  <For each={CONTACT_TYPES}>{(type) => <option>{type}</option>}</For>
                </select>
              </label>
              <Show when={contact()} keyed>
                {(type) => (
                  <label>
                    {type === 'Email' ? 'Email address' : `${type} handle`}
                    <input
                      name="contact-value"
                      type={type === 'Email' ? 'email' : 'text'}
                      autocomplete={type === 'Email' ? 'email' : 'off'}
                      maxlength={254}
                    />
                  </label>
                )}
              </Show>
              <div class="bug-public-options">
                <label>
                  <input name="share-contact" type="checkbox" /> Share contact info publicly
                </label>
                <label>
                  <input name="share-files" type="checkbox" /> Share uploaded files publicly
                </label>
              </div>
              <p class="bug-status">Your description is posted publicly on GitHub.</p>
              <button type="submit" class="pill primary">
                Submit report
              </button>
            </fieldset>
            <Show when={status()}>
              <p role="status">{status()}</p>
            </Show>
            <Show when={error()}>
              <p role="alert">{error()}</p>
            </Show>
          </form>
        }
      >
        {(result) => (
          <p role="status">
            Report saved. ID <code>{result().id}</code>
            <Show when={result().issueURL}>
              {(url) => (
                <>
                  {' '}
                  · <a href={url()}>View GitHub issue</a>
                </>
              )}
            </Show>
          </p>
        )}
      </Show>
    </section>
  );
}
