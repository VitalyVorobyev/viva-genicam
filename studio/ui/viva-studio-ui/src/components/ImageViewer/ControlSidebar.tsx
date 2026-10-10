import type { ReactNode } from "react";
import type { ParseXmlResponse } from "../../xml_model/uigraph";
import type { SfncGroup } from "../../device/types";
import { useSfncGroups } from "../../device/useSfncGroups";
import { isSectionApplicable } from "./sfncGroupUtils";
import { SidebarSection } from "./SidebarSection";
import { useSectionPersistence } from "./useSectionPersistence";
import { AcquisitionSection } from "./AcquisitionSection";
import { FeatureGroupSection } from "./FeatureGroupSection";

interface ControlSidebarProps {
  collapsed: boolean;
  onToggle: () => void;
  isConnected: boolean;
  isAcquiring: boolean;
  frameCount: number;
  externalModel: ParseXmlResponse | null;
  cameraModel: string | null;
}

const NO_NODES = {};

export function ControlSidebar({
  collapsed,
  onToggle,
  isConnected,
  isAcquiring,
  frameCount,
  externalModel,
  cameraModel,
}: ControlSidebarProps) {
  const groups = useSfncGroups();
  const sectionPersistence = useSectionPersistence(cameraModel);
  const nodesById = externalModel?.graph.nodes_by_name ?? NO_NODES;
  const visibleGroups = groups.filter((g) => isSectionApplicable(g, nodesById));

  function renderSectionContent(group: SfncGroup): ReactNode {
    if (group.id === "acquisition_control") {
      return (
        <AcquisitionSection
          isConnected={isConnected}
          isAcquiring={isAcquiring}
          frameCount={frameCount}
          nodes={nodesById}
        />
      );
    }
    return <FeatureGroupSection group={group} nodes={nodesById} isConnected={isConnected} />;
  }

  return (
    <aside
      className={`iv-sidebar${collapsed ? " iv-sidebar--collapsed" : ""}`}
      aria-label="Control sidebar"
    >
      <button
        type="button"
        className={`iv-sidebar__toggle${collapsed ? " iv-sidebar__toggle--collapsed" : ""}`}
        aria-label={collapsed ? "Show controls" : "Hide controls"}
        title={collapsed ? "Show controls" : "Hide controls"}
        onClick={onToggle}
      >
        <span className="iv-sidebar__toggle-arrow" />
        {!collapsed && <span className="iv-sidebar__toggle-label">Controls</span>}
      </button>

      {collapsed ? (
        <div className="iv-sidebar__rail">
          {visibleGroups.map((group) => (
            <button
              key={group.id}
              type="button"
              className="iv-sidebar__rail-icon"
              title={group.title}
              onClick={onToggle}
            >
              {group.icon}
            </button>
          ))}
        </div>
      ) : (
        <>
          {groups.length === 0 ? (
            <p className="sidebar-placeholder">Loading controls…</p>
          ) : visibleGroups.length === 0 ? (
            <p className="sidebar-placeholder">No applicable sections for this device.</p>
          ) : (
            visibleGroups.map((group) => (
              <SidebarSection
                key={group.id}
                title={group.title}
                icon={group.icon}
                defaultOpen={sectionPersistence.isOpen(group.id, group.default_open)}
                onToggle={(open) => sectionPersistence.setOpen(group.id, open)}
              >
                {renderSectionContent(group)}
              </SidebarSection>
            ))
          )}
        </>
      )}
    </aside>
  );
}
