import { defineConfig } from "@farm.js/core";

export default defineConfig({
  // Both pages are static. Each also exports `dynamic = "force-static"`, which is
  // what makes the build prerender it to HTML.
  routeRules: {
    "/": { render: "static" },
    "/setup": { render: "static" },
  },
});
