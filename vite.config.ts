import { defineConfig } from 'vite-plus';

// apps/web keeps its Solid-specific rules in its own Vite+ configuration.
const generated = [
  '**/node_modules/**',
  'target/**',
  '**/build/**',
  '**/dist/**',
  '**/dist-local-preview/**',
  'bindings/javascript/pkg/**',
  'apps/web/public/runtime/**',
  'apps/web/public/artifacts/**',
  'mods/lib/settings/upstream/**',
  '**/*.generated.*',
];

export default defineConfig({
  lint: {
    categories: { correctness: 'error' },
    options: { typeAware: true, typeCheck: true },
    ignorePatterns: generated,
  },
  fmt: {
    singleQuote: true,
    ignorePatterns: generated,
  },
});
