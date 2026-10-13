// Colors every code sample with Shiki and writes the HTML to
// src/content/highlighted.json. Runs before `dev` and `build`, so the pages
// import plain HTML strings and the highlighter stays out of the browser.
import { writeFileSync } from "node:fs";
import { SAMPLES } from "../src/content/samples.ts";
import { highlight } from "../src/lib/highlight.ts";

const out = Object.fromEntries(
  Object.entries(SAMPLES).map(([id, sample]) => [id, highlight(sample.code, sample.lang)]),
);
writeFileSync(new URL("../src/content/highlighted.json", import.meta.url), JSON.stringify(out, null, 2) + "\n");
console.log(`highlighted ${Object.keys(out).length} code samples`);
