import js from '@eslint/js';
import jsxA11y from 'eslint-plugin-jsx-a11y';
import reactCompiler from 'eslint-plugin-react-compiler';
import reactHooks from 'eslint-plugin-react-hooks';
import globals from 'globals';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  { ignores: ['dist/**', 'node_modules/**', 'src-tauri/**'] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ['src/**/*.{ts,tsx}', 'tests/**/*.{ts,tsx}', 'bench.ts', 'a11y.ts', 'isolation/**/*.js'],
    languageOptions: { globals: { ...globals.browser, ...globals.node }, parserOptions: { ecmaFeatures: { jsx: true } } },
    plugins: { 'react-hooks': reactHooks, 'react-compiler': reactCompiler, 'jsx-a11y': jsxA11y },
    rules: {
      ...reactHooks.configs.flat.recommended.rules,
      ...reactCompiler.configs.recommended.rules,
      ...jsxA11y.flatConfigs.recommended.rules,
      'no-restricted-syntax': ['error',
        { selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']", message: 'Render untrusted content through the sanitized Markdown component.' },
        { selector: "MemberExpression[property.name='innerHTML']", message: 'Do not write or read raw HTML.' },
        { selector: "CallExpression[callee.name='eval']", message: 'eval is not allowed.' },
      ],
    },
  },
);
