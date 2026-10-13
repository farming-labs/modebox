import { defineConfig } from "@farm.js/core";

export default defineConfig({
  // Every page is prerendered to static HTML at build time.
  routeRules: {
    "/": { render: "static" },
    "/setup": { render: "static" },
  },
});
