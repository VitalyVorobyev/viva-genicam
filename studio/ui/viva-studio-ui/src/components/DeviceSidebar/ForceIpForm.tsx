import { useState } from "react";
import { useToast } from "../../context/ToastContext";
import { formatErrorMessage } from "../../device/errorMessage";

/**
 * FORCEIP: give a discovered camera an address until it restarts. For a
 * camera on another subnet, which discovery finds but nothing can open
 * (backlog ST-37). The backend sends it out of the interface the camera
 * answered on.
 */
export function ForceIpForm({ mac, onDone }: { mac: string; onDone: () => void }) {
  const { addToast } = useToast();
  const [ip, setIp] = useState("");
  const [subnet, setSubnet] = useState("255.255.255.0");
  const [gateway, setGateway] = useState("0.0.0.0");
  const [busy, setBusy] = useState(false);

  const send = async () => {
    setBusy(true);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("force_ip", { mac, ip, subnet, gateway });
      addToast(
        "success",
        `Sent: the camera answers at ${ip} until it restarts, and appears there at the next scan`,
      );
      onDone();
    } catch (error) {
      addToast("error", `Setting the IP failed: ${formatErrorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="device-dd__force-ip"
      onSubmit={(e) => {
        e.preventDefault();
        void send();
      }}
    >
      <input
        className="iv-text-input"
        aria-label="Temporary IP address"
        placeholder="IP address"
        inputMode="decimal"
        value={ip}
        onChange={(e) => setIp(e.target.value)}
      />
      <input
        className="iv-text-input"
        aria-label="Subnet mask"
        placeholder="Subnet mask"
        inputMode="decimal"
        value={subnet}
        onChange={(e) => setSubnet(e.target.value)}
      />
      <input
        className="iv-text-input"
        aria-label="Default gateway"
        placeholder="Gateway"
        inputMode="decimal"
        value={gateway}
        onChange={(e) => setGateway(e.target.value)}
      />
      <button type="submit" className="btn btn--sm" disabled={busy || ip === ""}>
        Send
      </button>
    </form>
  );
}
