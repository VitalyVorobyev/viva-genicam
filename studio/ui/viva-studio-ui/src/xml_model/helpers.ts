import type { UiNode, UiNodeKind } from "./uigraph";

// Keep presentation helpers tiny and pure so we don't invent parallel models.
export function nodeDisplayName(node: UiNode): string {
  return node.display_name?.trim() || node.name;
}

export function nodeKindLabel(kind: UiNodeKind): string {
  if (typeof kind === "string") {
    return kind;
  }

  return `Unknown (${kind.Unknown.tag})`;
}

export function isUnknownKind(kind: UiNodeKind): kind is Exclude<UiNodeKind, string> {
  return typeof kind !== "string";
}

/**
 * Map a UiNodeKind to a CSS class key used for per-kind colour theming.
 *
 * Returns one of: "integer" | "float" | "boolean" | "string" | "enum" |
 * "command" | "register" | "category" | "unknown"
 */
export function nodeKindCssKey(kind: UiNodeKind): string {
  if (typeof kind !== "string") return "unknown";
  switch (kind) {
    case "Integer":     return "integer";
    case "Float":       return "float";
    case "Boolean":     return "boolean";
    case "String":      return "string";
    case "Enumeration": return "enum";
    case "Command":     return "command";
    case "Register":    return "register";
    case "Category":    return "category";
    default:            return "unknown";
  }
}

/**
 * Return the single-character ASCII icon for a given UiNodeKind.
 * Used in the category tree and search result rows.
 */
export function nodeKindIcon(kind: UiNodeKind): string {
  if (typeof kind !== "string") return "\u00B7";
  switch (kind) {
    case "Integer":     return "#";
    case "Float":       return "~";
    case "Boolean":     return "?";
    case "String":      return "\"";
    case "Enumeration": return "=";
    case "Command":     return ">";
    case "Register":    return "@";
    case "Category":    return "\u25A6";
    default:            return "\u00B7";
  }
}
