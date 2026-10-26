import { readFile } from 'node:fs/promises';
import { stripTypeScriptTypes } from 'node:module';

/** Compile standalone browser helpers without persisting generated JavaScript. */
export async function browserScript(path: URL): Promise<string> {
  return stripTypeScriptTypes(await readFile(path, 'utf8'));
}
