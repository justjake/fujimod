export interface PendingReport {
  blob: Blob;
  name: string;
  issueURL: string;
}

async function database(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open('fujimod-feedback-v1', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('reports');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

/** Commit the latest ZIP before allowing navigation away from the simulator. */
export async function storePendingReport(report: PendingReport): Promise<void> {
  const stored = {
    bytes: await report.blob.arrayBuffer(),
    name: report.name,
    issueURL: report.issueURL,
  };
  const db = await database();
  try {
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('reports', 'readwrite');
      tx.objectStore('reports').put(stored, 'latest');
      tx.oncomplete = () => resolve();
      tx.onerror = tx.onabort = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}

export async function readPendingReport(): Promise<PendingReport | undefined> {
  try {
    const db = await database();
    try {
      return await new Promise((resolve, reject) => {
        const tx = db.transaction('reports', 'readonly');
        const request = tx.objectStore('reports').get('latest');
        tx.oncomplete = () =>
          resolve(
            request.result && request.result.bytes instanceof ArrayBuffer
              ? {
                  blob: new Blob([request.result.bytes], { type: 'application/zip' }),
                  name: request.result.name,
                  issueURL: request.result.issueURL,
                }
              : undefined,
          );
        tx.onerror = tx.onabort = () => reject(tx.error);
      });
    } finally {
      db.close();
    }
  } catch {
    return undefined;
  }
}

export function deliverReport(report: PendingReport, download: (blob: Blob, name: string) => void) {
  download(report.blob, report.name);
  window.open(report.issueURL, '_blank', 'noopener,noreferrer');
}
