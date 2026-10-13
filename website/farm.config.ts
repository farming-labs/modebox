import { defineConfig } from "@farm.js/core";

export default defineConfig({
  // One page, prerendered to static HTML at build time.
  routeRules: {
    "/": { render: "static" },
  },
});
