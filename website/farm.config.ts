import { defineConfig } from "@farm.js/core";

export default defineConfig({
  // No generated Markdown copies of the pages.
  md: false,
  // Both pages are static. Each also exports `dynamic = "force-static"`, which is
  // what makes the build prerender it to HTML.
  routeRules: {
    "/": { render: "static" },
    "/setup": { render: "static" },
  },
});
