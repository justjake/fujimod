import { expect, test } from 'vite-plus/test';
import { scenarioFromUrl } from './scenarios';

test('a named body route selects its own firmware and preserves the graph RTC', () => {
  expect(scenarioFromUrl('?scenario=camera-rec&firmware=xm1-150')).toEqual({
    firmwareId: 'xm1-150',
    firmwarePath: '/local/firmware/xm1-150.dat',
    interactive: true,
    steps: [],
  });
  expect(scenarioFromUrl('?scenario=x70-lcd').firmwarePath).toBe('/local/firmware.dat');
});

test('body routes refuse missing and invalid firmware identifiers', () => {
  for (const id of ['', '../xm1-150', 'xm1-150/extra', 'xm1-150?other'])
    expect(() =>
      scenarioFromUrl('?scenario=camera-rec&firmware=' + encodeURIComponent(id)),
    ).toThrow('Invalid simulator firmware ID');
});
