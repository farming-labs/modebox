"use client";

import { useEffect, useRef } from "react";

/**
 * One listener for every server-rendered copy button on the page
 * (`<button data-copy="…">`), so they don't each need their own client root.
 */
export function CopyDelegate() {
  const live = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const onClick = async (event: MouseEvent) => {
      const button = (event.target as Element | null)?.closest<HTMLButtonElement>("button[data-copy]");
      if (!button) return;
      try {
        await navigator.clipboard.writeText(button.dataset.copy ?? "");
        button.textContent = "Copied";
        if (live.current) live.current.textContent = "Copied";
        setTimeout(() => {
          button.textContent = "Copy";
          if (live.current) live.current.textContent = "";
        }, 1500);
      } catch {
        // Clipboard access can be refused; the text stays selectable.
      }
    };
    document.addEventListener("click", onClick);
    return () => document.removeEventListener("click", onClick);
  }, []);
  return <span ref={live} className="sr-only" aria-live="polite" />;
}
