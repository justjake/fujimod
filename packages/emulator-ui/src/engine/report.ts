import { zipBlob } from '../archive';
export interface ReportSubmission {
  description: string;
  issue?: { url: string; fields: Record<string, string> };
}
/** Diagnostic bytes remain opaque; live keyframes belong to their running worker. */
export function errorReportZip(
  report: Record<string, unknown>,
  inspection?: ArrayBuffer,
  attachments: [string, Uint8Array<ArrayBuffer>][] = [],
): Promise<Blob> {
  const files: [string, Uint8Array<ArrayBuffer>][] = [
    ['report.json', new TextEncoder().encode(JSON.stringify(report, null, 2) + '\n')],
  ];
  if (inspection) files.push(['inspection.bin', new Uint8Array(inspection)]);
  return zipBlob(
    [...files, ...attachments].map(([path, bytes]) => ({ path, data: new Blob([bytes]) })),
  );
}
