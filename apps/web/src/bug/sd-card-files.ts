const bootFiles = new Set([
  'FFINIT.BIN',
  'FFBOOT.BIN',
  'FFDATA.BIN',
  'FFSYS.BIN',
  'FFUSER.BIN',
  'MARBLE_INIT.BIN',
  'FM_EXR2.BIN',
  'FM_EXR2.NEW',
]);

export interface ReportFile {
  file: File;
  path: string;
  included: boolean;
}

export function sdCardFiles(files: File[]): ReportFile[] {
  return files
    .flatMap((file) => {
      // The directory input prefixes each path with the selected volume name.
      const [, ...parts] = file.webkitRelativePath.split('/');
      if (
        !parts.length ||
        parts.some((part) => !part || part === '.' || part === '..' || part.includes('\\'))
      )
        return [];
      const [root, ...children] = parts.map((part) => part.toUpperCase());
      if (children.includes('DCIM')) return [];
      const fromBackup =
        root === 'BACKUP' && children.length === 1 && /^X70_\d{3}\.IMG$/.test(children[0]);
      const fromFujiMod = root === 'FUJIMOD' && children.length > 0;
      const fromBoot = children.length === 0 && bootFiles.has(root);
      return fromBackup || fromFujiMod || fromBoot
        ? [{ file, path: parts.join('/'), included: true }]
        : [];
    })
    .sort((a, b) => a.path.localeCompare(b.path));
}
