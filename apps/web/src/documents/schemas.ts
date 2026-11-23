import recipe from './recipe.schema.generated.json' with { type: 'json' };
import settings from './settings.schema.generated.json' with { type: 'json' };
import type { Schema } from './schema';

/** The camera's document schemas, generated from the mods' definitions. */
export const settingsSchema: Schema = settings;
export const recipeSchema: Schema = recipe;
export const documentSchemas: readonly Schema[] = [settingsSchema, recipeSchema];
