import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import Device from './Page';
import { deviceRequest, googleSignInUrl, resolveDevice, sessionUser } from '../auth/client';

vi.mock('../auth/client', () => ({
  sessionUser: vi.fn(),
  deviceRequest: vi.fn(),
  googleSignInUrl: vi.fn(),
  resolveDevice: vi.fn(),
}));

const user = { id: 'u1', email: 'a@b.c', name: 'A' };

async function mount(url: string) {
  history.replaceState(null, '', url);
  const host = document.createElement('div');
  document.body.append(host);
  const dispose = render(() => <Device />, host);
  await settle();
  return {
    host,
    dispose: () => {
      dispose();
      host.remove();
    },
  };
}

async function settle() {
  flush();
  await Promise.resolve();
  await Promise.resolve();
  flush();
}

function button(host: HTMLElement, name: string) {
  return [...host.querySelectorAll('button')].find((node) => node.textContent?.trim() === name);
}

afterEach(() => {
  vi.mocked(sessionUser).mockReset();
  vi.mocked(deviceRequest).mockReset();
  vi.mocked(googleSignInUrl).mockReset();
  vi.mocked(resolveDevice).mockReset();
});

test('a signed-in visitor approves the code the camera shows', async () => {
  vi.mocked(sessionUser).mockResolvedValue(user);
  vi.mocked(deviceRequest).mockResolvedValue({
    user_code: '2GK5QTNV',
    status: 'pending',
    client_id: 'x70-camera',
    scope: 'google.photos.append',
  });
  vi.mocked(resolveDevice).mockResolvedValue();
  const { host, dispose } = await mount('/device/?user_code=2GK5QTNV');
  try {
    expect(deviceRequest).toHaveBeenCalledWith('2GK5QTNV');
    expect(host.querySelector('h2')?.textContent).toBe('Fujifilm camera');
    expect(host.querySelector('.device-scopes')?.textContent).toBe('google.photos.append');

    button(host, 'Approve')!.click();
    await settle();

    expect(resolveDevice).toHaveBeenCalledWith('2GK5QTNV', true);
    expect(host.querySelector('.device-status')?.textContent).toBe('This camera can now connect.');
  } finally {
    dispose();
  }
});

test('a signed-out visitor is sent through Google with the code preserved', async () => {
  vi.mocked(sessionUser).mockResolvedValue(null);
  vi.mocked(googleSignInUrl).mockRejectedValue(new Error('No Google client'));
  const { host, dispose } = await mount('/device/?user_code=2GK5QTNV');
  try {
    const signIn = button(host, 'Sign in with Google');
    expect(signIn).toBeDefined();
    signIn!.click();
    await settle();

    expect(googleSignInUrl).toHaveBeenCalledWith('/device/?user_code=2GK5QTNV');
    expect(host.querySelector('.device-error')?.textContent).toBe('No Google client');
  } finally {
    dispose();
  }
});

test('a typed code is reviewed and kept in the address', async () => {
  vi.mocked(sessionUser).mockResolvedValue(user);
  vi.mocked(deviceRequest).mockResolvedValue({ user_code: 'ABCD1234', status: 'denied' });
  const { host, dispose } = await mount('/device/');
  try {
    const input = host.querySelector('input')!;
    input.value = 'abcd1234';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    flush();
    host
      .querySelector('form')!
      .dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await settle();

    expect(deviceRequest).toHaveBeenCalledWith('ABCD1234');
    expect(window.location.search).toBe('?user_code=ABCD1234');
    expect(host.querySelector('.device-status')?.textContent).toBe('This camera was denied.');
  } finally {
    dispose();
  }
});
