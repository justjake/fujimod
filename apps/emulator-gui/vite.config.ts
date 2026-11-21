import { defineConfig } from 'vite-plus';
import solid from '@solidjs/vite-plugin';

export default defineConfig({
  clearScreen: false,
  base: './',
  resolve: { dedupe: ['solid-js', '@solidjs/web', '@solidjs/signals'] },
  server: { port: 1420, strictPort: true },
  build: { target: 'es2022' },
  plugins: [solid()],
  fmt: { singleQuote: true },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.tsx'],
  },
});
