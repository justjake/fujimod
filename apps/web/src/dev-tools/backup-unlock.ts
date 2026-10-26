/** Cameras whose flash backup was made or skipped, remembered per serial number in local storage. */
export type BackupChoice = 'done' | 'skipped';

const KEY = 'fujimod.dev-tools.flash-backup';

function stored(): Record<string, BackupChoice> {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(KEY) ?? '{}');
    return value && typeof value === 'object' ? (value as Record<string, BackupChoice>) : {};
  } catch {
    return {};
  }
}

export function backupChoice(serial: string | undefined): BackupChoice | undefined {
  return serial ? stored()[serial] : undefined;
}

/** Remember `choice` for `serial`; a made backup is never downgraded to skipped. */
export function rememberBackup(serial: string | undefined, choice: BackupChoice) {
  if (!serial) return;
  const all = stored();
  if (all[serial] === 'done') return;
  localStorage.setItem(KEY, JSON.stringify({ ...all, [serial]: choice }));
}
