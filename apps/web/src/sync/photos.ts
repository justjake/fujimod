/** Google Photos Library API uploads for an append-only device token.
 *
 * A photo takes two requests: the raw bytes go to `/v1/uploads` and produce an
 * upload token, then `mediaItems:batchCreate` attaches that token to a file
 * name. Keeping the steps separate lets a client retry either one without
 * resending the bytes. */

export interface CameraPhoto {
  fileName: string;
  bytes: Uint8Array;
  contentType?: string;
}

export interface UploadedPhoto {
  fileName: string;
  mediaItemId?: string;
}

export interface PhotosRequest {
  /** Defaults to the Google Photos Library API endpoint. */
  baseUrl?: string;
  accessToken: string;
  fetch: typeof fetch;
}

export const photosBaseUrl = 'https://photoslibrary.googleapis.com';

export class PhotosUploadError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = 'PhotosUploadError';
  }
}

async function failure(response: Response, action: string): Promise<string> {
  const body = (await response.json().catch(() => undefined)) as
    | { error?: { message?: string } | string }
    | undefined;
  const detail = typeof body?.error === 'string' ? body.error : body?.error?.message;
  return detail ?? `Google Photos ${action} failed with ${response.status}.`;
}

interface BatchCreateResult {
  mediaItem?: { id?: string };
  status?: { code?: number; message?: string };
}

/** Uploads one photo and returns the library item Google created for it. */
export async function uploadPhoto(
  photo: CameraPhoto,
  request: PhotosRequest,
): Promise<UploadedPhoto> {
  const base = request.baseUrl ?? photosBaseUrl;
  const contentType = photo.contentType ?? 'application/octet-stream';
  const uploaded = await request.fetch(`${base}/v1/uploads`, {
    method: 'POST',
    headers: {
      authorization: `Bearer ${request.accessToken}`,
      'content-type': 'application/octet-stream',
      'x-goog-upload-protocol': 'raw',
      'x-goog-upload-content-type': contentType,
    },
    body: photo.bytes as unknown as BodyInit,
  });
  if (!uploaded.ok) throw new PhotosUploadError(await failure(uploaded, 'upload'), uploaded.status);
  const uploadToken = (await uploaded.text()).trim();
  if (!uploadToken)
    throw new PhotosUploadError('Google Photos returned an empty upload token.', uploaded.status);

  const created = await request.fetch(`${base}/v1/mediaItems:batchCreate`, {
    method: 'POST',
    headers: {
      authorization: `Bearer ${request.accessToken}`,
      'content-type': 'application/json',
    },
    body: JSON.stringify({
      newMediaItems: [{ simpleMediaItem: { fileName: photo.fileName, uploadToken } }],
    }),
  });
  if (!created.ok)
    throw new PhotosUploadError(await failure(created, 'batchCreate'), created.status);
  const body = (await created.json().catch(() => undefined)) as
    | { newMediaItemResults?: BatchCreateResult[] }
    | undefined;
  const result = body?.newMediaItemResults?.[0];
  if (!result)
    throw new PhotosUploadError('Google Photos returned no media item result.', created.status);
  if (result.status?.code)
    throw new PhotosUploadError(
      result.status.message ?? `Google Photos rejected ${photo.fileName}.`,
      created.status,
    );
  return { fileName: photo.fileName, mediaItemId: result.mediaItem?.id };
}
