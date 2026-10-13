# modebox website

The site at [modebox.farming-labs.dev](https://modebox.farming-labs.dev), built with [Farm.js](https://farmjs.dev). Two pages, both prerendered to static HTML: the home page (`src/app/page.tsx`) and the setup guide (`src/app/setup/page.tsx`, the same content as [`docs/integrations.md`](../docs/integrations.md)).

```bash
pnpm install
pnpm dev     # http://localhost:3000
pnpm build   # .farm/.output; on Vercel, Farm writes .vercel/output instead
```

Code samples live in `src/content/samples.ts`. Before `dev`, `build` and `type-check`, `scripts/highlight.ts` colors them with Shiki into `src/content/highlighted.json` (not committed), so the pages import plain HTML and the highlighter never reaches the browser. The figures in `src/components/fig-*.tsx` are plain SVG animated with CSS (`src/app/globals.css`).

Deploys go to the `farming-labs` team on Vercel, with this folder as the project root.
