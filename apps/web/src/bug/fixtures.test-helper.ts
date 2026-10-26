// TAR fixture created with Python tarfile (USTAR), gzip mtime=0.
import { gunzipSync } from 'node:zlib';
import { crc32 } from '@fujimod/emulator-ui/archive';
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
  crc32: crc32(tar).toString(16).padStart(8, '0'),
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
        c: value.crc32,
        f: value.files,
        a: value.archive,
      }),
    ),
  );
