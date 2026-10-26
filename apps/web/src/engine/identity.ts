/** Hash actual uploaded bytes; never a host path or generated version stamp. */
export async function digest(data: BufferSource) {
  return [...new Uint8Array(await crypto.subtle.digest('SHA-256', data))]
    .map((value) => value.toString(16).padStart(2, '0'))
    .join('');
}
