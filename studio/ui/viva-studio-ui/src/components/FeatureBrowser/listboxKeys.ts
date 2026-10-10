import type { KeyboardEvent } from "react";

/**
 * The keys a listbox promises: the arrows move focus to the next or previous
 * option, Home and End to the first and last (backlog ST-40). Enter and Space
 * already choose one, since each option is a button.
 */
export function onListboxKeyDown(e: KeyboardEvent<HTMLElement>): void {
  const options = Array.from(e.currentTarget.querySelectorAll<HTMLElement>('[role="option"]'));
  if (options.length === 0) return;
  const at = options.indexOf(document.activeElement as HTMLElement);
  let next: number;
  switch (e.key) {
    case "ArrowDown":
      next = Math.min(at + 1, options.length - 1);
      break;
    case "ArrowUp":
      next = Math.max(at - 1, 0);
      break;
    case "Home":
      next = 0;
      break;
    case "End":
      next = options.length - 1;
      break;
    default:
      return;
  }
  e.preventDefault();
  options[next].focus();
}
