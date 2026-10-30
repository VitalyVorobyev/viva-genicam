import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ToastProvider } from "../../../context/ToastContext";
import type { FeatureState } from "../../../device/types";
import { ToastContainer } from "../../Layout/ToastContainer";
import { MAX_DISPLAY_BYTES } from "./registerHex";
import { RegisterEditor } from "./RegisterEditor";

function register(length: number, access_mode = "RW"): FeatureState {
  return {
    value: { kind: "register", length },
    access_mode,
    kind: "Register",
    is_implemented: true,
    is_available: true,
  };
}

/** A register device: reads return what was last written. */
function device(initial: number[]) {
  let bytes = initial;
  const writes: unknown[] = [];
  mockIPC((cmd, args) => {
    const a = args as { nodeName: string; data?: number[] };
    if (cmd === "read_register") return bytes;
    if (cmd === "write_register") {
      writes.push(a);
      bytes = a.data ?? bytes;
      return null;
    }
    return undefined;
  });
  return writes;
}

function show(state: FeatureState) {
  return render(
    <ToastProvider>
      <RegisterEditor name="DeviceUserBuffer" liveState={state} />
      <ToastContainer />
    </ToastProvider>,
  );
}

const ASCII = Array.from("Viva GenICam 0.1", (c) => c.charCodeAt(0));

describe("RegisterEditor", () => {
  it("shows the bytes with offsets and ASCII", async () => {
    device(ASCII);
    show(register(16));
    const dump = await screen.findByLabelText("Register bytes");
    expect(dump.textContent).toContain("00000000  56 69 76 61");
    expect(dump.textContent).toContain("Viva GenICam 0.1");
  });

  it("writes exactly the register's length and reads it back", async () => {
    const writes = device(ASCII);
    show(register(16));
    const user = userEvent.setup();
    await screen.findByLabelText("Register bytes");
    const input = screen.getByLabelText("New bytes (hex)");
    const write = screen.getByRole("button", { name: "Write" });

    await user.type(input, "01 02 03");
    expect(screen.getByText(/3 of 16 bytes/)).toBeTruthy();
    expect(write).toHaveProperty("disabled", true);

    await user.clear(input);
    await user.type(input, "00".repeat(15) + "FF");
    expect(write).toHaveProperty("disabled", false);
    await user.click(write);
    expect(writes).toEqual([{ nodeName: "DeviceUserBuffer", data: [...Array(15).fill(0), 255] }]);
    expect(await screen.findByText(/00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 FF/)).toBeTruthy();
  });

  it("copies the current bytes into the editor", async () => {
    device(ASCII);
    show(register(16));
    const user = userEvent.setup();
    await screen.findByLabelText("Register bytes");
    await user.click(screen.getByRole("button", { name: "Copy to editor" }));
    expect((screen.getByLabelText("New bytes (hex)") as HTMLTextAreaElement).value).toBe(
      "56 69 76 61 20 47 65 6E 49 43 61 6D 20 30 2E 31",
    );
  });

  it("offers no editor for a read-only register, and no dump for a write-only one", async () => {
    device(ASCII);
    const { unmount } = show(register(16, "RO"));
    await screen.findByLabelText("Register bytes");
    expect(screen.queryByLabelText("New bytes (hex)")).toBeNull();
    expect(screen.getByText(/cannot be written/)).toBeTruthy();
    unmount();

    show(register(16, "WO"));
    expect(screen.getByText(/Write-only register/)).toBeTruthy();
    expect(screen.queryByLabelText("Register bytes")).toBeNull();
    expect(screen.getByLabelText("New bytes (hex)")).toBeTruthy();
  });

  it("draws only the start of a large register, but reads it whole", async () => {
    device(new Array(MAX_DISPLAY_BYTES * 2).fill(0x41));
    show(register(MAX_DISPLAY_BYTES * 2, "RO"));
    const dump = await screen.findByLabelText("Register bytes");
    expect(dump.textContent?.split("\n")).toHaveLength(MAX_DISPLAY_BYTES / 16);
    expect(screen.getByText(new RegExp(`first ${MAX_DISPLAY_BYTES} of ${MAX_DISPLAY_BYTES * 2} bytes`))).toBeTruthy();
  });
});
