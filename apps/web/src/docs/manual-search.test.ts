import { describe, expect, it } from 'vite-plus/test';
import { searchManual } from './manual-search';

const first = (query: string) => searchManual(query)[0]?.id;

describe('searchManual', () => {
  it('finds settings inside pages', () => {
    expect(first('halation')).toBe('film-effects#shooting-menu--film-effects--halation');
    expect(first('date stamp')).toBe('film-effects#shooting-menu--film-effects--date-stamp');
  });

  it('finds a setting by one of its choices', () => {
    expect(first('speckle')).toBe('noise-reduction#shooting-menu--noise-reduction');
    expect(first('luma')).toBe('noise-reduction#shooting-menu--noise-reduction');
  });

  it('matches words as they are typed and tolerates a typo in longer words', () => {
    expect(first('spec')).toBe('noise-reduction#shooting-menu--noise-reduction');
    expect(first('halaton')).toBe('film-effects#shooting-menu--film-effects--halation');
  });

  it('matches short words whole', () => {
    expect(first('nr')).toBe('noise-reduction#shooting-menu--noise-reduction');
    expect(new Set(searchManual('af').flatMap((hit) => hit.terms))).toEqual(new Set(['af']));
  });

  it('requires every word', () => {
    expect(searchManual('halation speckle')).toEqual([]);
  });

  it('links to the section and highlights the matched words', () => {
    const [hit] = searchManual('halation');
    expect(hit.href).toBe('/docs/manual/film-effects/#shooting-menu--film-effects--halation');
    expect(hit.trail).toEqual(['SHOOTING MENU', 'FILM EFFECTS']);
    expect(hit.terms).toContain('halation');
  });
});
