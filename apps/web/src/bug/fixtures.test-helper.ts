// TAR fixture created with Python tarfile (USTAR), gzip mtime=0.
import { gunzipSync } from 'node:zlib';
import { base64url, type ReportLink } from './report-link';
export const tar = Uint8Array.from(
  gunzipSync(
    Buffer.from(
      'H4sIAAAAAAAC/+3PV1SOARzH8SdEyIoKGdkq1VPeHiuy95aMaEl5Wy/VS2U07D2KspVdykjDLDN701ZmVkOy53s4x4U7F9z0/d78zvnf/M/HRekm91Q4G3soXE2c5F7Cv0hUJclkP1fVHyuJksXv26+7mShJ5oK+KPyHlD6+jt6q90L5rEq9ZiZd+o92UsxavG77gROX0h+//la9fitzq8HjXHyClm/YffjUtexnbyvUamQg9Ro+0W3GvNVbYpLP3cp79bFyXX3jzv2sHb1mLlobtf/4xXuPSr5W021p1m3Q2MnegcvW74pPvZpVUKZWU6+tRc9hE+TT567aHJ109ub9lx/UtZq269R3lINnwMKwyLhjF+4+LP5SVaeF2HXgGOdpc5ZG7DyUciXz6RuhRsM2sh5DbacoQ1Zu2pt45kbui/eV6jQx6thnpL2H/4LQbbFH0+48KPqsod3c1HKAzaSps5eE7zh48nLGk9Lvmg1at+8+ZLyrb/CKjXsSTl/Pef6uYu3Ghh16j7Bz95u/Zuu+I+dv5xd+wo8fP378+PHjx48fP378+PHjx48fP378+PHjx48fP378+PHjx48fP378+PHjx48fP378+PHjx48fP378+PHjx48fP378+PHjx48fP378+PHjx48fP378+PHjx48fP378+PHjx48fP378+PGXH79ARERERERERERE9Jf9AE8tvHkAeAAA',
      'base64',
    ),
  ),
);
export const link: ReportLink = {
  v: 1,
  firmware: 'x70-110',
  commit: 'a'.repeat(8),
  path: 'A:\\FUJIMOD\\REPORTS\\R1234567.TAR',
  bytes: tar.length,
  files: '403a0000',
};
export const hash = (value = link) =>
  '#?report=' +
  base64url(
    new TextEncoder().encode(
      JSON.stringify({
        v: value.v,
        b: value.commit,
        r: value.path.match(/R([a-fA-F0-9]{7})\.TAR$/)?.[1] ?? '',
        n: value.bytes,
        f: value.files,
        a: value.archive,
      }),
    ),
  );
/** A USTAR report like the camera writes: `files`, then a manifest. */
export function reportTar(
  files: Record<string, Uint8Array>,
  capturedAt?: string,
  recorded: Record<string, unknown> = {},
) {
  const encoder = new TextEncoder();
  const manifest = encoder.encode(
    JSON.stringify({
      format_version: 1,
      firmware: 'x70-110',
      ...(capturedAt === undefined ? {} : { captured_at: capturedAt }),
      ...recorded,
      files: Object.entries(files).map(([path, data]) => ({
        path,
        bytes: data.length,
        status: 0,
      })),
    }),
  );
  const parts: Uint8Array[] = [];
  for (const [name, data] of [...Object.entries(files), ['manifest.json', manifest] as const]) {
    const header = new Uint8Array(512);
    const field = (offset: number, text: string) => header.set(encoder.encode(text), offset);
    field(0, name);
    field(100, '0000644\0');
    field(124, data.length.toString(8).padStart(11, '0') + '\0');
    field(136, '00000000000\0');
    header[156] = 48;
    field(257, 'ustar\0');
    header.fill(32, 148, 156);
    field(
      148,
      header
        .reduce((sum, byte) => sum + byte, 0)
        .toString(8)
        .padStart(6, '0') + '\0 ',
    );
    parts.push(header, data, new Uint8Array((512 - (data.length % 512)) % 512));
  }
  parts.push(new Uint8Array(1024));
  const bytes = new Uint8Array(parts.reduce((total, part) => total + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    bytes.set(part, offset);
    offset += part.length;
  }
  return bytes;
}
