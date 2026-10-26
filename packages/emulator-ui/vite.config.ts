import { defineConfig } from 'vite-plus';
import solid from '@solidjs/vite-plugin';
import { uiLint } from './lint';

export default defineConfig({
  plugins: [solid()],
  lint: uiLint,
  fmt: { singleQuote: true },
  test: {
    pool: 'forks',
    projects: [
      {
        extends: true,
        test: {
          name: 'unit',
          environment: 'node',
          include: ['src/**/*.test.ts'],
        },
      },
      {
        extends: true,
        test: {
          name: 'components',
          environment: 'jsdom',
          include: ['src/**/*.test.tsx'],
        },
      },
    ],
  },
});
