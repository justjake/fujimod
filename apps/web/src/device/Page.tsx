import { For, Show, createMemo, createSignal, onSettled } from 'solid-js';
import { SiteHeader } from '../ui/site-header';
import {
  deviceRequest,
  googleSignInUrl,
  resolveDevice,
  sessionUser,
  type DeviceRequest,
  type DeviceStatus,
  type SessionUser,
} from '../auth/client';
import '../home/site.css';
import '../navigation.css';
import './device.css';

const STATUS: Record<DeviceStatus, string> = {
  pending: 'This camera is waiting for your approval.',
  approved: 'This camera can now connect.',
  denied: 'This camera was denied.',
};

/** The camera shows a code, and opens this page with it when it can show a link. */
function codeFromLocation(): string {
  if (typeof window === 'undefined') return '';
  const url = new URL(window.location.href);
  return (url.searchParams.get('user_code') ?? url.searchParams.get('code') ?? '')
    .trim()
    .toUpperCase();
}

function message(caught: unknown): string {
  return caught instanceof Error ? caught.message : String(caught);
}

export default function Device() {
  const [code, setCode] = createSignal(codeFromLocation());
  const [session, setSession] = createSignal<SessionUser | null>();
  const [request, setRequest] = createSignal<DeviceRequest>();
  const [status, setStatus] = createSignal<DeviceStatus>();
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal('');

  const scopes = createMemo(() => (request()?.scope ?? '').split(' ').filter(Boolean));

  async function review(userCode: string) {
    setError('');
    setBusy(true);
    try {
      const current = await sessionUser();
      setSession(current);
      if (!current) return;
      const found = await deviceRequest(userCode);
      setRequest(found);
      setStatus(found.status);
    } catch (caught) {
      setError(message(caught));
    } finally {
      setBusy(false);
    }
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    const typed = code().trim().toUpperCase();
    if (!typed) return;
    setCode(typed);
    setRequest(undefined);
    setStatus(undefined);
    const url = new URL(window.location.href);
    url.searchParams.set('user_code', typed);
    history.replaceState(null, '', url);
    void review(typed);
  }

  async function signIn() {
    setError('');
    setBusy(true);
    try {
      location.assign(await googleSignInUrl(`/device/?user_code=${encodeURIComponent(code())}`));
    } catch (caught) {
      setError(message(caught));
      setBusy(false);
    }
  }

  async function decide(approve: boolean) {
    setError('');
    setBusy(true);
    try {
      await resolveDevice(code(), approve);
      setStatus(approve ? 'approved' : 'denied');
    } catch (caught) {
      setError(message(caught));
    } finally {
      setBusy(false);
    }
  }

  onSettled(() => {
    const pending = code();
    if (pending) void review(pending);
    else void sessionUser().then(setSession, () => setSession(null));
  });

  return (
    <div class="shell device">
      <SiteHeader root="../" />
      <main>
        <h1>Camera sign-in</h1>
        <form class="device-code" onSubmit={submit}>
          <label for="device-code">Code</label>
          <input
            id="device-code"
            value={code()}
            onInput={(event) => setCode(event.currentTarget.value.toUpperCase())}
            autocomplete="off"
            autocapitalize="characters"
            spellcheck={false}
            enterkeyhint="go"
            maxlength={16}
          />
          <button type="submit" class="pill" disabled={busy() || !code().trim()}>
            Continue
          </button>
        </form>

        <Show when={error()}>{(text) => <p class="device-error">{text()}</p>}</Show>

        <Show when={session() === null && code().trim()}>
          <button
            type="button"
            class="pill primary device-sign-in"
            disabled={busy()}
            onClick={() => void signIn()}
          >
            Sign in with Google
          </button>
        </Show>

        <Show when={request()}>
          <section class="device-request">
            <h2>{request()!.client_id ?? 'Camera'}</h2>
            <Show when={scopes().length}>
              <ul class="device-scopes">
                <For each={scopes()}>{(scope) => <li>{scope}</li>}</For>
              </ul>
            </Show>
            <Show
              when={status() === 'pending' && session()}
              fallback={<p class="device-status">{STATUS[status()!]}</p>}
            >
              <div class="device-actions">
                <button
                  type="button"
                  class="pill primary"
                  disabled={busy()}
                  onClick={() => void decide(true)}
                >
                  Approve
                </button>
                <button
                  type="button"
                  class="pill"
                  disabled={busy()}
                  onClick={() => void decide(false)}
                >
                  Deny
                </button>
              </div>
            </Show>
          </section>
        </Show>
      </main>
    </div>
  );
}
