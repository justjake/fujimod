import { expect, test } from 'vite-plus/test';
import { Direction, Layer, Level, exportLog } from './log';

test('the exported log is JSON Lines without payload bytes', () => {
  const text = exportLog([
    {
      time: 0,
      layer: Layer.Ff80,
      level: Level.Debug,
      direction: Direction.In,
      message: 'Read memory · 0x0 · 16384 B',
      data: new Uint8Array(256),
    },
    {
      time: 1000,
      layer: Layer.Console,
      level: Level.Info,
      direction: Direction.Out,
      message: 'help',
    },
  ]);
  expect(text.endsWith('\n')).toBe(true);
  const lines = text
    .trimEnd()
    .split('\n')
    .map((line) => JSON.parse(line) as Record<string, unknown>);
  expect(lines).toHaveLength(2);
  expect(lines[0]).toMatchObject({
    time: '1970-01-01T00:00:00.000Z',
    message: 'Read memory · 0x0 · 16384 B',
  });
  expect(lines[0]).not.toHaveProperty('data');
  expect(lines[1]).toMatchObject({ layer: 'Console', message: 'help' });
});
