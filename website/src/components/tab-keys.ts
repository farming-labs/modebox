import type { KeyboardEvent } from "react";

/**
 * Arrow keys, Home and End move between the tabs of a tablist and select the
 * one they land on, as in the WAI-ARIA tabs pattern. Only the selected tab is
 * in the Tab order (`tabIndex={selected ? 0 : -1}`).
 */
export function onTabKeys(
  event: KeyboardEvent<HTMLElement>,
  ids: string[],
  active: string,
  select: (id: string) => void,
) {
  const i = ids.indexOf(active);
  const next =
    event.key === "ArrowRight"
      ? (i + 1) % ids.length
      : event.key === "ArrowLeft"
        ? (i - 1 + ids.length) % ids.length
        : event.key === "Home"
          ? 0
          : event.key === "End"
            ? ids.length - 1
            : -1;
  if (next < 0) return;
  event.preventDefault();
  select(ids[next]);
  event.currentTarget.querySelectorAll<HTMLElement>('[role="tab"]')[next]?.focus();
}
