import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import { SidebarSection } from "./SidebarSection";

function Section({ initiallyOpen }: { initiallyOpen: boolean }) {
  const [open, setOpen] = useState(initiallyOpen);
  return (
    <SidebarSection id="exposure_gain" title="Exposure & Gain" icon="sun" open={open} onToggle={setOpen}>
      <p>Exposure controls</p>
    </SidebarSection>
  );
}

describe("SidebarSection", () => {
  it("opens and closes from its header, and says which", async () => {
    render(<Section initiallyOpen={false} />);
    const header = screen.getByRole("button", { name: "Exposure & Gain" });
    expect(header.getAttribute("aria-expanded")).toBe("false");
    expect(screen.getByText("Exposure controls").closest("[hidden]")).not.toBeNull();

    await userEvent.click(header);
    expect(header.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("Exposure controls").closest("[hidden]")).toBeNull();

    await userEvent.click(header);
    expect(header.getAttribute("aria-expanded")).toBe("false");
  });

  it("names the body it controls", () => {
    render(<Section initiallyOpen={true} />);
    const header = screen.getByRole("button", { name: "Exposure & Gain" });
    const body = document.getElementById(header.getAttribute("aria-controls") ?? "");
    expect(body?.textContent).toBe("Exposure controls");
  });
});
