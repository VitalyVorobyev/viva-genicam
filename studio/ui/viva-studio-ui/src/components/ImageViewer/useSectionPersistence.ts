import { useState } from "react";

export function buildStorageKey(cameraModel: string | null): string {
  return `iv-sections-${cameraModel ?? "__default__"}`;
}

export function readStoredSections(
  cameraModel: string | null,
): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(buildStorageKey(cameraModel));
    if (raw === null) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      return {};
    }
    return parsed as Record<string, boolean>;
  } catch {
    return {};
  }
}

export function writeStoredSections(
  cameraModel: string | null,
  sections: Record<string, boolean>,
): void {
  try {
    localStorage.setItem(buildStorageKey(cameraModel), JSON.stringify(sections));
  } catch {
    // Private-browsing or quota exceeded — ignore
  }
}

export interface SectionPersistence {
  isOpen(id: string, defaultOpen: boolean): boolean;
  setOpen(id: string, open: boolean): void;
}

export function useSectionPersistence(
  cameraModel: string | null,
): SectionPersistence {
  const [sections, setSections] = useState<Record<string, boolean>>(() =>
    readStoredSections(cameraModel),
  );

  // A new camera brings its own sections, in the same render rather than
  // one effect later, so no section shows the previous camera's state.
  const [prevModel, setPrevModel] = useState(cameraModel);
  if (cameraModel !== prevModel) {
    setPrevModel(cameraModel);
    setSections(readStoredSections(cameraModel));
  }

  function isOpen(id: string, defaultOpen: boolean): boolean {
    return id in sections ? sections[id] : defaultOpen;
  }

  function setOpen(id: string, open: boolean): void {
    setSections((prev) => {
      const next = { ...prev, [id]: open };
      writeStoredSections(cameraModel, next);
      return next;
    });
  }

  return { isOpen, setOpen };
}
