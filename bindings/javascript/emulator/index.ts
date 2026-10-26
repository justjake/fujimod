export type * from './camera.generated.ts';
export {
  createCamera,
  instantiateEmulatorModule,
  isGuestExecutionError,
  loadEmulatorModule,
  parseEmulatorManifest,
  type Camera,
  type EmulatorManifest,
  type EmulatorModule,
  type EmulatorModuleFactory,
  type Frame,
  type ModuleFactoryOptions,
  type PreparedInputs,
  type Status,
} from './module.ts';
import { loadEmulatorModule } from './module.ts';
import type { ModuleOptions } from './camera.generated.ts';
export { HostTimings } from './host-timings.ts';

/** Load the generated runtime artifacts staged beside this package entry. */
export function loadBundledEmulatorModule(
  options: ModuleOptions = {},
  fetcher: typeof fetch = fetch,
) {
  return loadEmulatorModule(
    new URL('./camera.mjs', import.meta.url),
    new URL('./camera.wasm', import.meta.url),
    options,
    fetcher,
  );
}
