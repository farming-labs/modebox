// Server-side syntax highlighting. The page is prerendered, so this runs at
// build time and the browser only receives colored HTML.
import json from "@shikijs/langs/json";
import python from "@shikijs/langs/python";
import rust from "@shikijs/langs/rust";
import typescript from "@shikijs/langs/typescript";
import vesper from "@shikijs/themes/vesper";
import { createHighlighterCoreSync } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

export type Lang = "json" | "ts" | "py" | "rs";

const NAMES: Record<Lang, string> = {
  json: "json",
  ts: "typescript",
  py: "python",
  rs: "rust",
};

const highlighter = createHighlighterCoreSync({
  themes: [vesper],
  langs: [json, python, rust, typescript],
  engine: createJavaScriptRegexEngine(),
});

export function highlight(code: string, lang: Lang): string {
  return highlighter.codeToHtml(code, { lang: NAMES[lang], theme: "vesper" });
}
