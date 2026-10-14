import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { counters } from "../../test/streamCounters";
import type { StreamCounters } from "../../device/types";
import type { StreamTransport } from "../../device/acquisitionStatus";
import { StatisticsSection } from "./StatisticsSection";

function section(
  stats: StreamCounters | null,
  { transport = "gige", active = true }: { transport?: StreamTransport; active?: boolean } = {},
) {
  return (
    <StatisticsSection
      isConnected={true}
      active={active}
      fps={active ? 241 : null}
      stats={stats}
      transport={transport}
      displayed={{ frames: 300, fps: 30 }}
    />
  );
}

function value(group: string, label: string): HTMLElement {
  return within(screen.getByRole("group", { name: group })).getByLabelText(label);
}

describe("StatisticsSection", () => {
  it("shows GigE Vision's packets and resends", () => {
    render(section(counters({ frames_complete: 2410, packets_received: 1_234_567, resend_requested: 5 })));
    expect(value("Frames", "Received").textContent).toBe("2,410");
    expect(value("Packets", "Received").textContent).toBe("1,234,567");
    expect(value("Resend", "Requested").textContent).toBe("5");
    expect(screen.queryByRole("group", { name: "USB" })).toBeNull();
  });

  // A USB3 Vision stream has no packets to resend: a row of zeros there
  // would read as a healthy link rather than as nothing to count.
  it("shows USB3 Vision's transfers and no resends", () => {
    render(section(counters({ packets_received: 9000, transfer_errors: 2 }), { transport: "u3v" }));
    expect(value("USB", "Transfers").textContent).toBe("9,000");
    expect(value("USB", "Transfer errors").textContent).toBe("2");
    expect(screen.queryByRole("group", { name: "Resend" })).toBeNull();
    expect(screen.queryByRole("group", { name: "Packets" })).toBeNull();
  });

  it("highlights loss, and only loss", () => {
    render(section(counters({ frames_complete: 10, frames_missed: 3 })));
    expect(value("Frames", "Missed").className).toContain("iv-stat--warn");
    expect(value("Frames", "Incomplete").className).not.toContain("iv-stat--warn");
    expect(value("Frames", "Received").className).not.toContain("iv-stat--warn");
  });

  // The camera's rate and the viewer's are two numbers, and the viewer's is
  // capped: at 241 fps the drawn count climbs at 30 a second.
  it("keeps what was received apart from what was displayed", () => {
    render(section(counters({ frames_complete: 2410 })));
    expect(value("Rates", "Frame rate").textContent).toBe("241.0 fps");
    expect(value("Rates", "Display rate").textContent).toBe("30.0 fps");
    expect(value("Frames", "Displayed").textContent).toBe("300");
  });

  it("keeps the last values after Stop, and says so", () => {
    render(section(counters({ frames_complete: 900 }), { active: false }));
    expect(screen.getByText("Stopped")).toBeTruthy();
    expect(value("Frames", "Received").textContent).toBe("900");
    expect(value("Rates", "Display rate").textContent).toBe("—");
  });

  it("asks for an acquisition before there is anything to count", () => {
    render(section(null, { active: false }));
    expect(screen.getByText("Start acquisition to see statistics.")).toBeTruthy();
  });
});
