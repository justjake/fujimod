import catalogData from './features.generated.json';

/** Inline Markdown from the docs: text, bold, italic and links. */
export type Inline = string | { b: Inline[] } | { i: Inline[] } | { href: string; text: Inline[] };
export type Credits = { premiere: boolean; inspired: Inline[][]; thanks: Inline[][] };
/** A hover card: one control, its options and what FujiMod changed. */
export type Card = {
  title: string;
  path: string[];
  summary: string | null;
  choices: Record<string, string>;
  related: { card: string; reason: string }[];
  changes: string | null;
  credits: Credits;
  choice_credits: Record<string, Credits>;
};
export type Choice = { label: string; stock: boolean; icon: string | null };
export type Control = {
  title: string;
  primary: boolean;
  card: string | null;
  choices: Choice[];
  actions: string[];
};
/** A catalog tile: one feature, its controls and the cards its preview may come from. */
export type Feature = {
  id: string;
  group: string;
  icon: string;
  title: string;
  path: string[];
  summary: string;
  card: string | null;
  media_cards: string[];
  controls: Control[];
  credits: Credits;
};
/** Generated feature descriptions and menu options. */
export type Catalog = {
  schema: 1;
  groups: string[];
  features: Feature[];
  cards: Record<string, Card>;
  menus: MenuRow[];
};
/** A menu row FujiMod adds, its card when documented and the feature it belongs to. */
export type MenuRow = {
  title: string;
  path: string[];
  card: string | null;
  feature: string | null;
};

export const catalog = catalogData as Catalog;
