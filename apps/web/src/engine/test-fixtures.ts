import type { EngineConfig } from '@fujimod/emulator-ui/engine/api';
export function testConfig(sceneInput = true): EngineConfig {
  return { firmwareId: 'test', bundle: 'runtime/', sceneInput };
}
