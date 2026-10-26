export const uiLint = {
  jsPlugins: ['eslint-plugin-solid'],
  settings: { solid: { version: 2 } },
  overrides: [
    {
      files: ['**/*.{jsx,tsx}'],
      rules: {
        'solid/reactivity': 'error',
        'solid/no-destructure': 'error',
        'solid/components-return-once': 'error',
        'solid/no-react-deps': 'error',
        'solid/removed-api': 'error',
        'solid/no-single-arg-create-effect': 'error',
        'solid/no-accessor-as-prop': 'error',
        'solid/no-store-mutation-outside-setter': 'error',
        'solid/no-write-in-pure-computation': 'error',
      } as const,
    },
  ],
  options: { typeAware: true, typeCheck: true },
  ignorePatterns: [
    'build/**',
    'dist/**',
    'dist-local-preview/**',
    'public/runtime/**',
    'public/artifacts/**',
  ],
};
