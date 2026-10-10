import { useEffect, useState } from "react";

/**
 * Whether an element is on screen, or near it. A list row passes the result to
 * `useFeature`, so a category of hundreds of features, or a broad search,
 * reads from the camera only the rows the user can see (backlog ST-40; Codex
 * review of #172). Without `IntersectionObserver` every row counts as shown.
 */
export function useInView<T extends Element>(): [(element: T | null) => void, boolean] {
  const observable = typeof IntersectionObserver !== "undefined";
  const [element, setElement] = useState<T | null>(null);
  const [inView, setInView] = useState(!observable);

  useEffect(() => {
    if (!element || !observable) return;
    const observer = new IntersectionObserver(
      (entries) => {
        const entry = entries[entries.length - 1];
        if (entry) setInView(entry.isIntersecting);
      },
      // Read a little ahead of the scroll, so values are there on arrival.
      { rootMargin: "200px 0px" },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [element, observable]);

  return [setElement, inView];
}
