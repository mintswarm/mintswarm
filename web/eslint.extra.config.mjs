import { defineConfig } from "eslint/config";
import react from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";

// The React pack's rules, taken in by eslint.config.mjs.
export default defineConfig(
  react.configs.flat.recommended,
  react.configs.flat["jsx-runtime"],
  reactHooks.configs.flat["recommended-latest"],
  {
    settings: { react: { version: "detect" } },
    rules: {
      "react/prop-types": "off",
      "react-hooks/exhaustive-deps": "error",
    },
  },
);
