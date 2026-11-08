import { expect, test } from 'vite-plus/test';
import { cardDownloadAllowed } from './download-policy';

test('card firmware files obey independent per-firmware release switches', () => {
  for (const sd_card of [false, true])
    for (const installable of [false, true]) {
      const firmware = { downloads: { sd_card, installable }, update_filename: 'FWUP0009.DAT' };
      expect(cardDownloadAllowed('FFBoot.BIN', firmware)).toBe(sd_card);
      expect(cardDownloadAllowed('ffsys.bin', firmware)).toBe(sd_card);
      expect(cardDownloadAllowed('fwup0009.dat', firmware)).toBe(installable);
      expect(cardDownloadAllowed('FPUPDATE.DAT', firmware)).toBe(true);
      expect(cardDownloadAllowed('FPUPDATE.DAT', { downloads: firmware.downloads })).toBe(
        installable,
      );
      expect(cardDownloadAllowed('DSCF0001.JPG', firmware)).toBe(true);
    }
  expect(cardDownloadAllowed('ffboot.bin')).toBe(false);
  expect(cardDownloadAllowed('FPUPDATE.DAT')).toBe(false);
  expect(cardDownloadAllowed('ffboot.bin.bak')).toBe(true);
});
