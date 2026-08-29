import js from "@eslint/js";
import { defineConfig } from "eslint/config";
import security from "eslint-plugin-security";
import svelte from "eslint-plugin-svelte";
import globals from "globals";
import ts from "typescript-eslint";

import svelteConfig from "./svelte.config.js";

export default defineConfig(
  {
    ignores: [".svelte-kit/**", "android/**", "build/**"],
  },
  js.configs.recommended,
  ...ts.configs.recommended,
  ...svelte.configs.recommended,
  security.configs.recommended,
  {
    languageOptions: {
      globals: {
        ...globals.browser,
        ...globals.node,
      },
    },
    rules: {
      // Keep the initial lint adoption focused on correctness and security.
      // These legacy-style rules need a separate, repository-wide cleanup.
      "@typescript-eslint/no-unused-vars": "off",
      "svelte/no-navigation-without-resolve": "off",
      "svelte/require-each-key": "off",
    },
  },
  {
    files: ["**/*.svelte", "**/*.svelte.js", "**/*.svelte.ts"],
    languageOptions: {
      parserOptions: {
        extraFileExtensions: [".svelte"],
        parser: ts.parser,
        projectService: true,
        svelteConfig,
      },
    },
  },
);
