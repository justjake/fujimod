import MiniSearch, { type SearchResult } from 'minisearch';
import manual from './manual.generated.json' with { type: 'json' };
import type { ManualPage } from './manual-data.ts';
import { manualPagePath } from './manual-routes.ts';

/** A manual page, or one setting or heading on it. */
type Entry = {
  id: string;
  href: string;
  title: string;
  /** The setting's menu path, or the page a heading is on, for the result's breadcrumb. */
  trail: string[];
  /** The menu path and any other place the setting appears, for matching. */
  path: string;
  choices: string;
  text: string;
};

export type ManualHit = {
  id: string;
  href: string;
  title: string;
  trail: readonly string[];
  /** A sentence from the entry's text holding a matched word, or its first. */
  snippet: string;
  /** Matched words as they are indexed, for highlighting. */
  terms: readonly string[];
};

const fields = ['title', 'path', 'choices', 'text'] as const;

/** Lowercase, without accents; the typographic minus matches a hyphen. */
export function normalizeTerm(term: string) {
  return term
    .normalize('NFKD')
    .replace(/\p{Mn}/gu, '')
    .replace(/−/g, '-')
    .toLowerCase();
}

/** Markdown as plain text. */
function plain(markdown: string) {
  return markdown
    .replace(/!\[[^\]]*\]\([^)]*\)/g, '')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/[*_`#>]/g, '')
    .replace(/^\s*[-+]\s+/gm, '')
    .replace(/\s+/g, ' ')
    .trim();
}

function entries(): Entry[] {
  const pages: Record<string, ManualPage> = manual.pages;
  const list: Entry[] = [];
  for (const [slug, page] of Object.entries(pages)) {
    const href = manualPagePath(slug);
    list.push({
      id: slug,
      href,
      title: page.title,
      trail: [],
      path: '',
      choices: '',
      text: plain(`${page.summary} ${page.introduction}`),
    });
    for (const section of page.sections) {
      const paths = section.paths ?? [];
      list.push({
        id: `${slug}#${section.anchor}`,
        href: `${href}#${section.anchor}`,
        title: section.title,
        trail: section.path?.length ? [...section.path] : [page.title],
        path: paths.map((parts) => parts.join(' ')).join(' '),
        choices: (section.options ?? []).map((option) => option.label).join(' '),
        text: plain(
          [
            section.summary ?? '',
            ...(section.options ?? []).map((option) => option.summary ?? ''),
            section.markdown ?? '',
          ].join(' '),
        ),
      });
    }
  }
  return list;
}

let index: { search: MiniSearch<Entry>; byId: Map<string, Entry> } | undefined;

function built() {
  if (index) return index;
  const list = entries();
  const search = new MiniSearch<Entry>({
    fields: [...fields],
    storeFields: [],
    processTerm: normalizeTerm,
    searchOptions: {
      combineWith: 'AND',
      // Words of three letters or more match as typed so far; short words
      // ("nr", "af") match whole words only.
      prefix: (term) => term.length >= 3,
      // One typo in five letters, from five letters up.
      fuzzy: (term) => (term.length >= 5 ? 0.2 : false),
      boost: { title: 4, choices: 2, path: 1.5, text: 1 },
    },
  });
  search.addAll(list);
  index = { search, byId: new Map(list.map((entry) => [entry.id, entry])) };
  return index;
}

function snippet(text: string, terms: readonly string[]) {
  const sentences = text.match(/[^.!?]+[.!?]*/g) ?? [];
  const words = (sentence: string) => sentence.split(/[^\p{L}\p{N}-]+/u).map(normalizeTerm);
  const found = sentences.find((sentence) => words(sentence).some((word) => terms.includes(word)));
  return (found ?? sentences[0] ?? '').trim();
}

/** The best matches for a query, an exact title first. */
export function searchManual(query: string, limit = 20): ManualHit[] {
  const wanted = normalizeTerm(query.trim());
  if (!wanted) return [];
  const { search, byId } = built();
  const exact = (result: SearchResult) =>
    normalizeTerm(byId.get(String(result.id))!.title) === wanted ? 1 : 0;
  return search
    .search(query)
    .sort((a, b) => exact(b) - exact(a) || b.score - a.score)
    .slice(0, limit)
    .map((result) => {
      const entry = byId.get(String(result.id))!;
      return {
        id: entry.id,
        href: entry.href,
        title: entry.title,
        trail: entry.trail,
        snippet: snippet(entry.text, result.terms),
        terms: result.terms,
      };
    });
}
