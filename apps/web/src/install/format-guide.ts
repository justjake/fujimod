/** How to format an SD card as FAT32 on each desktop system. */
export const SYSTEMS = ['macOS', 'Windows', 'Linux'] as const;
export type System = (typeof SYSTEMS)[number];

const BACK_UP = 'Check that it’s okay to erase the SD card. Back up any files you want to keep.';

export const FORMAT_STEPS: Record<System, string[]> = {
  macOS: [
    BACK_UP,
    'Open Disk Utility and choose View → Show All Devices.',
    'Select the SD card itself, not its volume, and click Erase.',
    'Choose Format: MS-DOS (FAT) and Scheme: Master Boot Record, then Erase.',
  ],
  Windows: [
    BACK_UP,
    'Use a card of 32 GB or less; Windows formats larger cards as exFAT only.',
    'In File Explorer, right-click the SD card and choose Format.',
    'Choose File system: FAT32, then Start.',
  ],
  Linux: [
    BACK_UP,
    'Open Disks and select the SD card.',
    'Choose Format Disk with Compatible with all systems (MBR/DOS).',
    'Create one partition with Type: For use with all systems and devices (FAT).',
  ],
};

export function detectSystem(agent: string): System {
  if (/Windows/.test(agent)) return 'Windows';
  if (/Linux|CrOS/.test(agent) && !/Android/.test(agent)) return 'Linux';
  return 'macOS';
}
