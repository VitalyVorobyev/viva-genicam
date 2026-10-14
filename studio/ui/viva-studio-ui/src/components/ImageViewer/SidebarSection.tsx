import type { ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import { SectionIcon } from "./sectionIcons";

interface SidebarSectionProps {
  /** The group's id; the section's element is `iv-section-<id>`. */
  id: string;
  title: string;
  /** A name from `sectionIcons.ts`. */
  icon: string;
  children: ReactNode;
  open: boolean;
  onToggle: (open: boolean) => void;
}

/**
 * One collapsible section of the control sidebar. Its open state is the
 * caller's, so the rail can open a section and a new camera brings its own.
 */
export function SidebarSection({ id, title, icon, children, open, onToggle }: SidebarSectionProps) {
  const bodyId = `iv-section-${id}-body`;
  return (
    <section className="sidebar-section" id={`iv-section-${id}`}>
      <button
        type="button"
        className="sidebar-section__header"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => onToggle(!open)}
      >
        <SectionIcon name={icon} className="sidebar-section__icon" size={14} />
        <span className="sidebar-section__label">{title}</span>
        <ChevronDown
          className={`sidebar-section__chevron${open ? "" : " sidebar-section__chevron--closed"}`}
          size={16}
          aria-hidden="true"
        />
      </button>
      <div className="sidebar-section__body" id={bodyId} hidden={!open}>
        {children}
      </div>
    </section>
  );
}
