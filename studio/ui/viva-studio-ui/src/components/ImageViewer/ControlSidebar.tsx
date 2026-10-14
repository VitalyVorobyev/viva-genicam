import { useEffect, useRef, type CSSProperties, type ReactNode } from "react";
import { PanelRightClose, PanelRightOpen } from "lucide-react";
import type { ParseXmlResponse } from "../../xml_model/uigraph";
import type { SfncGroup } from "../../device/types";
import { useSfncGroups } from "../../device/useSfncGroups";
import { useSplitter } from "../Layout/useSplitter";
import { isSectionApplicable } from "./sfncGroupUtils";
import { SidebarSection } from "./SidebarSection";
import { SectionIcon } from "./sectionIcons";
import { useSectionPersistence } from "./useSectionPersistence";
import { AcquisitionSection } from "./AcquisitionSection";
import { FeatureGroupSection } from "./FeatureGroupSection";
import { StatisticsSection, type StatisticsSectionProps } from "./StatisticsSection";

interface ControlSidebarProps {
  collapsed: boolean;
  onToggle: () => void;
  isConnected: boolean;
  isAcquiring: boolean;
  externalModel: ParseXmlResponse | null;
  cameraModel: string | null;
  deviceId: string | null;
  /** The stream's counters, for the Statistics section. */
  statistics: Omit<StatisticsSectionProps, "isConnected">;
}

const NO_NODES = {};

/**
 * The Statistics section, which the UI adds after Acquisition Control (or
 * first, for a camera without one): it shows the stream, not features.
 */
const STATISTICS = { id: "statistics", title: "Statistics", icon: "activity", defaultOpen: true };

/**
 * The image viewer's control panel: the `sfnc-groups.json` sections this
 * camera has, and the stream's statistics, in a panel the user can resize,
 * or collapse to a rail of section icons. A rail icon opens the panel at its
 * section.
 */
export function ControlSidebar({
  collapsed,
  onToggle,
  isConnected,
  isAcquiring,
  externalModel,
  cameraModel,
  deviceId,
  statistics,
}: ControlSidebarProps) {
  const groups = useSfncGroups();
  const sectionPersistence = useSectionPersistence(cameraModel);
  const nodesById = externalModel?.graph.nodes_by_name ?? NO_NODES;
  const visibleGroups = groups.filter((g) => isSectionApplicable(g, nodesById));
  const { size: width, handleProps: splitterProps } = useSplitter({
    storageKey: "viva-studio:iv-sidebar-width",
    defaultSize: 280,
    minSize: 240,
    maxSize: 480,
    paneAfter: true,
  });

  // The section a rail icon asked for, scrolled to the top of the panel
  // once the panel shows it. Only the panel scrolls: `scrollIntoView` would
  // also move the viewer's clipped ancestors.
  const scrollTarget = useRef<string | null>(null);
  const bodyRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const body = bodyRef.current;
    if (collapsed || !body || scrollTarget.current === null) return;
    const section = document.getElementById(`iv-section-${scrollTarget.current}`);
    if (!section) return;
    body.scrollTop = section.offsetTop;
    scrollTarget.current = null;
  });

  const openAt = (id: string) => {
    sectionPersistence.setOpen(id, true);
    scrollTarget.current = id;
    if (collapsed) onToggle();
  };

  function renderSectionContent(group: SfncGroup): ReactNode {
    if (group.id === "acquisition_control") {
      return (
        <AcquisitionSection
          isConnected={isConnected}
          isAcquiring={isAcquiring}
          nodes={nodesById}
          deviceId={deviceId}
        />
      );
    }
    return <FeatureGroupSection group={group} nodes={nodesById} isConnected={isConnected} />;
  }

  // Statistics goes after Acquisition Control, or first.
  const statsAfter = visibleGroups.findIndex((g) => g.id === "acquisition_control") + 1;
  const sections: Array<{ id: string; title: string; icon: string }> = [...visibleGroups];
  sections.splice(statsAfter, 0, STATISTICS);
  const groupsById = new Map(visibleGroups.map((g) => [g.id, g]));

  function renderSection(id: string): ReactNode {
    if (id === STATISTICS.id) {
      return (
        <SidebarSection
          key={id}
          id={id}
          title={STATISTICS.title}
          icon={STATISTICS.icon}
          open={sectionPersistence.isOpen(id, STATISTICS.defaultOpen)}
          onToggle={(open) => sectionPersistence.setOpen(id, open)}
        >
          <StatisticsSection isConnected={isConnected} {...statistics} />
        </SidebarSection>
      );
    }
    const group = groupsById.get(id);
    if (!group) return null;
    return (
      <SidebarSection
        key={group.id}
        id={group.id}
        title={group.title}
        icon={group.icon}
        open={sectionPersistence.isOpen(group.id, group.default_open)}
        onToggle={(open) => sectionPersistence.setOpen(group.id, open)}
      >
        {renderSectionContent(group)}
      </SidebarSection>
    );
  }

  const toggleLabel = collapsed ? "Show controls" : "Hide controls";
  const style = collapsed ? undefined : ({ "--iv-sidebar-width": `${width}px` } as CSSProperties);

  return (
    <>
      {!collapsed && <div {...splitterProps} aria-label="Resize controls" />}
      <aside
        className={`iv-sidebar${collapsed ? " iv-sidebar--collapsed" : ""}`}
        aria-label="Control sidebar"
        style={style}
      >
        <div className="iv-sidebar__head">
          {!collapsed && <span className="iv-sidebar__title">Controls</span>}
          <button
            type="button"
            className="iv-sidebar__toggle"
            aria-label={toggleLabel}
            title={toggleLabel}
            onClick={onToggle}
          >
            {collapsed ? (
              <PanelRightOpen size={16} aria-hidden="true" />
            ) : (
              <PanelRightClose size={16} aria-hidden="true" />
            )}
          </button>
        </div>

        {collapsed ? (
          <div className="iv-sidebar__rail">
            {sections.map((section) => (
              <button
                key={section.id}
                type="button"
                className="iv-sidebar__rail-icon"
                aria-label={section.title}
                title={section.title}
                onClick={() => openAt(section.id)}
              >
                <SectionIcon name={section.icon} size={16} />
              </button>
            ))}
          </div>
        ) : (
          <div className="iv-sidebar__body" ref={bodyRef}>
            {sections.map((section) => renderSection(section.id))}
            {groups.length === 0 ? (
              <p className="sidebar-placeholder">Loading controls…</p>
            ) : (
              visibleGroups.length === 0 && (
                <p className="sidebar-placeholder">No applicable sections for this device.</p>
              )
            )}
          </div>
        )}
      </aside>
    </>
  );
}
