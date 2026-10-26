/** Encode UTF-8 through an ordinary buffer before copying into Wasm shared memory. */
export function encodeIntoSharedMemory(
  encoder: TextEncoder,
  text: string,
  target: Uint8Array<ArrayBufferLike>,
): TextEncoderEncodeIntoResult {
  const temporary = new Uint8Array(target.byteLength);
  const result = encoder.encodeInto(text, temporary);
  target.set(temporary.subarray(0, result.written));
  return result;
}

/** wasm-bindgen's generated UTF-8 encoder must also work with pthread memory. */
export function sharedStringEncoding(source: string): string {
  const call = /\bcachedTextEncoder\.encodeInto\(arg,\s*view\)/g;
  if ([...source.matchAll(call)].length !== 1)
    throw Error('Expected one generated wasm-bindgen UTF-8 encoder.');
  return (
    source.replace(call, 'encodeIntoSharedMemory(cachedTextEncoder,arg,view)') +
    '\n' + encodeIntoSharedMemory.toString() + '\n'
  );
}
