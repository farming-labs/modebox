"use client";

import { useEffect } from "react";

/**
 * One listener for every server-rendered copy button on the page
 * (`<button data-copy="…">`), so they don't each need their own client root.
 */
export function CopyDelegate() {
  useEffect(() => {
    const onClick = async (event: MouseEvent) => {
      const button = (event.target as Element | null)?.closest<HTMLButtonElement>("button[data-copy]");
      if (!button) return;
      try {
        await navigator.clipboard.writeText(button.dataset.copy ?? "");
        button.textContent = "Copied";
        setTimeout(() => {
          button.textContent = "Copy";
        }, 1500);
      } catch {
        // Clipboard access can be refused; the text stays selectable.
      }
    };
    document.addEventListener("click", onClick);
    return () => document.removeEventListener("click", onClick);
  }, []);
  return null;
}
