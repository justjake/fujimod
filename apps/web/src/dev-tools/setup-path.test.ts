import { expect, test } from 'vite-plus/test';
import { CAMERA_STAGES, SETUP_PATH, stepNumber } from './setup-path';

test('Get started follows the bring-up graph order', () => {
  const order = CAMERA_STAGES.map((stage) => stage.id);
  const positions = SETUP_PATH.map((stage) => order.indexOf(stage));
  expect(positions.every((position) => position >= 0)).toBe(true);
  expect([...positions].sort((a, b) => a - b)).toEqual(positions);
  expect(SETUP_PATH.map(stepNumber)).toEqual([1, 2, 3, 4]);
});

test('debug mode waits for the flash backup, as the bring-up graph gates it', () => {
  const debug = CAMERA_STAGES.find((stage) => stage.id === 'camera-debug');
  expect(debug?.requires).toContain('camera-backup');
});
