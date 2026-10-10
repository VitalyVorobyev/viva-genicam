/**
 * The GVSP packet size the user set for a camera's link, kept per device id
 * in this browser's storage. `null` — the default — lets the backend
 * negotiate one from the NIC's MTU; a number is a ceiling it stays under
 * (backlog ST-22, issue #112).
 */
const key = (deviceId: string) => `viva-studio:packet-size:${deviceId}`;

/** The smallest and largest sizes GigE Vision's 16-bit register can hold. */
export const PACKET_SIZE_MIN = 576;
export const PACKET_SIZE_MAX = 65535;

export function loadPacketSize(deviceId: string): number | null {
  try {
    const raw = localStorage.getItem(key(deviceId));
    const size = raw === null ? NaN : Number(raw);
    return Number.isInteger(size) && size >= PACKET_SIZE_MIN && size <= PACKET_SIZE_MAX
      ? size
      : null;
  } catch {
    return null;
  }
}

export function savePacketSize(deviceId: string, size: number | null): void {
  try {
    if (size === null) localStorage.removeItem(key(deviceId));
    else localStorage.setItem(key(deviceId), String(size));
  } catch {
    // Storage may be unavailable; the setting then lasts this session only.
  }
}
