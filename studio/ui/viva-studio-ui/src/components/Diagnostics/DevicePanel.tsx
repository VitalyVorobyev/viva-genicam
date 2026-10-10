import { useEffect, useState } from "react";
import { useToast } from "../../context/ToastContext";
import { formatErrorMessage } from "../../device/errorMessage";
import { useFeature } from "../../device/FeatureStoreContext";
import type { NetworkConfig } from "../../device/types";
import { isTauri } from "../../tauri";

/** SFNC device-information features, shown when the camera has them. */
const IDENTITY: ReadonlyArray<readonly [label: string, feature: string]> = [
  ["Vendor", "DeviceVendorName"],
  ["Model", "DeviceModelName"],
  ["Serial", "DeviceSerialNumber"],
  ["Firmware", "DeviceFirmwareVersion"],
  ["Version", "DeviceVersion"],
  ["User ID", "DeviceUserID"],
];

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

/**
 * The connected camera: who it is and where it is on the network — what a
 * user reads out when filing a report — and its persistent IP address
 * (backlog ST-37).
 */
export function DevicePanel({ connected }: { connected: boolean }) {
  if (!connected) return null;
  return (
    <section className="device-panel" aria-label="Connected camera">
      <span className="diagnostics-tab__title">Camera</span>
      <dl className="device-panel__info">
        {IDENTITY.map(([label, feature]) => (
          <IdentityRow key={feature} label={label} feature={feature} />
        ))}
      </dl>
      <NetworkSection />
    </section>
  );
}

function IdentityRow({ label, feature }: { label: string; feature: string }) {
  const state = useFeature(feature);
  // Nothing arrives for a feature the camera does not have.
  if (!state?.is_implemented || state.value === null || typeof state.value === "object") {
    return null;
  }
  return (
    <>
      <dt>{label}</dt>
      <dd>{String(state.value)}</dd>
    </>
  );
}

type Network =
  | { kind: "loading" }
  | { kind: "ready"; config: NetworkConfig }
  | { kind: "unavailable"; reason: string };

function NetworkSection() {
  const [network, setNetwork] = useState<Network>({ kind: "loading" });

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    invoke<NetworkConfig>("get_network_config").then(
      (config) => !cancelled && setNetwork({ kind: "ready", config }),
      (error) => !cancelled && setNetwork({ kind: "unavailable", reason: formatErrorMessage(error) }),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  if (network.kind === "loading") return null;
  if (network.kind === "unavailable") {
    return <p className="device-panel__note">IP configuration: {network.reason}</p>;
  }
  const { config } = network;
  return (
    <>
      <dl className="device-panel__info">
        {config.mac !== "" && (
          <>
            <dt>MAC</dt>
            <dd>{config.mac}</dd>
          </>
        )}
        <dt>IP address</dt>
        <dd>{config.current_ip}</dd>
      </dl>
      <PersistentIpForm config={config} />
    </>
  );
}

/**
 * The address the camera takes at power-up instead of asking DHCP or picking
 * a link-local one. The camera applies it from its next restart.
 */
function PersistentIpForm({ config }: { config: NetworkConfig }) {
  const { addToast } = useToast();
  const [ip, setIp] = useState(config.persistent_ip);
  const [subnet, setSubnet] = useState(config.persistent_subnet);
  const [gateway, setGateway] = useState(config.persistent_gateway);
  const [busy, setBusy] = useState(false);

  const save = async () => {
    setBusy(true);
    try {
      await invoke("set_persistent_ip", { ip, subnet, gateway });
      addToast("success", `Saved ${ip}: the camera uses it from its next restart`);
    } catch (error) {
      addToast("error", `Setting the persistent IP failed: ${formatErrorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="device-panel__form"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <span className="device-panel__form-title">Persistent IP</span>
      <AddressField label="Persistent IP address" value={ip} onChange={setIp} />
      <AddressField label="Subnet mask" value={subnet} onChange={setSubnet} />
      <AddressField label="Default gateway" value={gateway} onChange={setGateway} />
      <button type="submit" className="btn btn--sm" disabled={busy}>
        Save to camera
      </button>
    </form>
  );
}

function AddressField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <input
      type="text"
      className="iv-text-input device-panel__address"
      aria-label={label}
      placeholder={label}
      inputMode="decimal"
      value={value}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}
