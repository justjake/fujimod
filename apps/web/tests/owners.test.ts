/** Camera owners: each contributor once, owning bodies the generation list names. */
import { expect, test } from 'vite-plus/test';
import generations from '../devices/generations.json';
import owners from '../devices/owners.json';

interface Owner {
  name: string;
  github: string;
  bodies: string[];
}

test('owners have unique GitHub names and own known bodies', () => {
  const list: Owner[] = owners,
    bodies = new Set(generations.flat().flatMap((group) => group.models)),
    names = list.map((owner) => owner.github.toLowerCase());
  expect(new Set(names).size).toBe(names.length);
  for (const owner of list) {
    expect(owner.name.trim()).not.toBe('');
    expect(owner.github).toMatch(/^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?$/);
    expect(
      owner.bodies.filter((body) => !bodies.has(body)),
      owner.github,
    ).toEqual([]);
    expect(new Set(owner.bodies).size, owner.github).toBe(owner.bodies.length);
  }
});
