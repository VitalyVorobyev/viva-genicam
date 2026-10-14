import { useId, type ReactNode } from "react";
import { Lock } from "lucide-react";

/** The ids a row hands its control, so the label and the reason reach it. */
export interface RowIds {
  /** For the control's `id`: the row's label points at it. */
  id: string;
  /** For the control's `aria-describedby`: why it is blocked, if it is. */
  describedBy: string | undefined;
}

interface InspectorRowProps {
  label: string;
  /** Why the control cannot be used now, or `null` if it can. */
  blocked: string | null;
  /** A hint that does not block the control, shown as the row's tooltip. */
  hint?: string;
  /** The feature the row draws, for `data-feature`. */
  feature?: string;
  children: (ids: RowIds) => ReactNode;
}

/**
 * One row of the image-viewer inspector: the label on the left, the control
 * in a fixed-width column on the right, so every control in the panel lines
 * up whatever its kind.
 *
 * A blocked row is drawn the same way for every kind: dimmed, with a lock by
 * the label, and the reason as the row's tooltip and the control's
 * accessible description.
 */
export function InspectorRow({ label, blocked, hint, feature, children }: InspectorRowProps) {
  const id = useId();
  const reasonId = `${id}-reason`;
  return (
    <div
      className={`iv-row feature-control${blocked ? " iv-row--blocked" : ""}`}
      data-feature={feature}
      title={blocked ?? hint}
    >
      <label className="iv-row__label" htmlFor={id} title={blocked ? `${label} — ${blocked}` : label}>
        {blocked && <Lock className="iv-row__lock" size={12} aria-hidden="true" />}
        <span className="iv-row__label-text">{label}</span>
      </label>
      <div className="iv-row__control">
        {children({ id, describedBy: blocked ? reasonId : undefined })}
      </div>
      {blocked && (
        <span id={reasonId} hidden>
          {blocked}
        </span>
      )}
    </div>
  );
}
