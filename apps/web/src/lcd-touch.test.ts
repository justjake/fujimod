import { expect, test } from 'vite-plus/test';
import { lcdTouchPosition } from './lcd-touch';

test('maps pointer positions within the LCD surface, including its edges', () => {
  const bounds = { left: 10, top: 20, width: 360, height: 240 };
  expect(lcdTouchPosition(10, 20, bounds, 720, 480)).toEqual({ x: 0, y: 0 });
  expect(lcdTouchPosition(190, 140, bounds, 720, 480)).toEqual({ x: 360, y: 240 });
  expect(lcdTouchPosition(370, 260, bounds, 720, 480)).toEqual({ x: 719, y: 479 });
  expect(lcdTouchPosition(500, 400, bounds, 720, 480)).toEqual({ x: 719, y: 479 });
  expect(lcdTouchPosition(0, 0, bounds, 720, 480)).toEqual({ x: 0, y: 0 });
});
