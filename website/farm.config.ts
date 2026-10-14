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
  // Everything the site loads comes from its own origin. With `nonce`, Farm
  // adds a hash for each inline script of a prerendered page. Styles allow
  // inline because the highlighted code colors its tokens with style attributes.
  security: {
    csp: {
      nonce: true,
      directives: {
        defaultSrc: "'self'",
        scriptSrc: "'self'",
        styleSrc: ["'self'", "'unsafe-inline'"],
        imgSrc: ["'self'", "data:"],
        fontSrc: "'self'",
        connectSrc: "'self'",
        objectSrc: "'none'",
        baseUri: "'self'",
        formAction: "'none'",
        frameAncestors: "'none'",
      },
    },
  },
  headers: () => [
    {
      source: "/*",
      headers: [
        { key: "X-Content-Type-Options", value: "nosniff" },
        { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
        { key: "Permissions-Policy", value: "camera=(), microphone=(), geolocation=()" },
      ],
    },
  ],
});
