import { createElement } from "react";
import {
  Activity,
  Bookmark,
  Crop,
  Network,
  Palette,
  Play,
  SlidersHorizontal,
  Sun,
  Zap,
  type LucideIcon,
} from "lucide-react";

/**
 * The icons `sfnc-groups.json` may name for a section. Unicode glyphs drew
 * differently on every OS (⚡ as a colour emoji), so a group names one of
 * these instead; an unknown name gets a neutral icon. A Rust test
 * (`commands/sfnc_groups.rs`) keeps the bundled file to these names.
 */
const SECTION_ICONS: Record<string, LucideIcon> = {
  play: Play,
  sun: Sun,
  crop: Crop,
  zap: Zap,
  bookmark: Bookmark,
  network: Network,
  palette: Palette,
  // The Statistics section, which the UI adds itself.
  activity: Activity,
};

/** The icon a section names, drawn decoratively. */
export function SectionIcon({ name, size, className }: { name: string; size: number; className?: string }) {
  return createElement(SECTION_ICONS[name] ?? SlidersHorizontal, {
    size,
    className,
    "aria-hidden": true,
  });
}
