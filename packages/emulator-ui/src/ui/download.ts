export function downloadBlob(blob: Blob, name: string) {
  if (import.meta.env.DEV)
    window.dispatchEvent(
      new CustomEvent('simulator-download', {
        detail: { name, bytes: blob.size },
      }),
    );
  const url = URL.createObjectURL(blob),
    a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60000);
}
