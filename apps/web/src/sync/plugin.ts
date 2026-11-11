/** The camera's Google Photos sync plugin.
 *
 * One run signs the camera in once, takes the owner's Google token from the
 * Worker, and uploads every photo the source yields. The source decides where
 * photos come from: a camera's SD card, a USB session, or an emulator frame. */

import {
  googleAccessToken,
  pollDeviceToken,
  requestDeviceCode,
  type DeviceCode,
} from './device.ts';
import { PhotosUploadError, uploadPhoto, type CameraPhoto, type UploadedPhoto } from './photos.ts';

export interface PhotoSource {
  photos(): AsyncIterable<CameraPhoto> | Iterable<CameraPhoto>;
}

export interface CameraSyncOptions {
  /** Origin of the FujiMod Worker that authorizes devices. */
  serverUrl: string;
  source: PhotoSource;
  clientId?: string;
  scope?: string;
  photosBaseUrl?: string;
  fetch?: typeof fetch;
  /** Shows the user code while the camera waits for approval. */
  onUserCode?: (code: DeviceCode) => void | Promise<void>;
  sleep?: (milliseconds: number) => Promise<void>;
  now?: () => number;
}

export interface SyncReport {
  uploaded: UploadedPhoto[];
}

export async function syncCameraPhotos(options: CameraSyncOptions): Promise<SyncReport> {
  const http = options.fetch ?? fetch;
  const request = {
    serverUrl: options.serverUrl,
    clientId: options.clientId ?? 'x70-camera',
    scope: options.scope ?? 'google.photos.append',
    fetch: http,
  };
  const code = await requestDeviceCode(request);
  await options.onUserCode?.(code);
  const deviceToken = await pollDeviceToken(request, code, {
    sleep: options.sleep,
    now: options.now,
  });
  let google = await googleAccessToken({
    serverUrl: options.serverUrl,
    deviceToken,
    fetch: http,
  });

  const upload = (photo: CameraPhoto) =>
    uploadPhoto(photo, {
      accessToken: google.accessToken,
      baseUrl: options.photosBaseUrl,
      fetch: http,
    });

  const uploaded: UploadedPhoto[] = [];
  for await (const photo of options.source.photos()) {
    let result: UploadedPhoto;
    try {
      result = await upload(photo);
    } catch (failure) {
      // Google invalidates an access token mid-sync; refresh it once.
      if (!(failure instanceof PhotosUploadError) || failure.status !== 401) throw failure;
      google = await googleAccessToken({
        serverUrl: options.serverUrl,
        deviceToken,
        fetch: http,
      });
      result = await upload(photo);
    }
    uploaded.push(result);
  }
  return { uploaded };
}
