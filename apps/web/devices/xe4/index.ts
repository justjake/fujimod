import panelData from './panel.json' with { type: 'json' };
import type { DevicePanel } from '@fujimod/emulator-ui/ui/layout';

/** X-E4's hand-arranged panel for the existing generic web control renderer. */
export const panel: DevicePanel = panelData as DevicePanel;
