import type { ReportLink } from './report-link';
import { resolveShareUrl } from '../files/share-url';
import { sharingBase, sharingUrl } from '../sharing-server';
export interface ReportReceipt {
  id: string;
  issueURL?: string;
}
export interface UploadReceipt extends ReportReceipt {
  shareUrl: string;
}
export type UploadReport = (
  archive: Blob,
  link: ReportLink,
  description: string,
  id: string,
) => Promise<UploadReceipt>;
/** Called only by the explicit upload action. The QR fragment never leaves the browser. */
export const uploadReport: UploadReport = async (archive, link, description, id) => {
  const body = new FormData();
  body.set('report', archive, link.path.split('\\').at(-1)!);
  body.set('metadata', JSON.stringify({ ...link, archive: undefined }));
  body.set('description', description);
  const receipt = await submitReport(body, id);
  const shareUrl = resolveShareUrl(receipt.shareUrl, sharingBase());
  if (!shareUrl) throw Error('The server did not confirm that your report was saved.');
  return { ...receipt, shareUrl };
};

export async function submitReport(
  body: FormData,
  id: string,
): Promise<ReportReceipt & { shareUrl?: string }> {
  const response = await fetch(sharingUrl('/issue-reports'), {
    method: 'POST',
    body,
    headers: { 'Idempotency-Key': id },
    credentials: 'omit',
  });
  if (!response.ok)
    throw Error(
      response.status === 503
        ? 'Report uploads are not configured yet. Your report is still on this device.'
        : 'Upload failed. Your report is still on this device. Try again.',
    );
  const receipt = (await response.json()) as ReportReceipt & { shareUrl?: string };
  if (
    receipt.id !== id ||
    (receipt.issueURL &&
      !/^https:\/\/github\.com\/justjake\/fujimod\/issues\/[1-9][0-9]*$/.test(receipt.issueURL))
  )
    throw Error('The server did not confirm that your report was saved.');
  return receipt;
}
