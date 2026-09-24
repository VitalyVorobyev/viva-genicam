import type { DeviceInfo } from "./types";

/**
 * The name that tells this device apart from another of the same model: the
 * user-defined name, else the serial, else the address (#137). Falls back to
 * `name` only for a service that sends none of the three.
 */
export function deviceLabel(d: DeviceInfo): string {
  return d.user_name || d.serial || d.ip || d.name;
}

/**
 * The secondary line under {@link deviceLabel}: the model, serial and address,
 * leaving out whichever one the label already shows.
 */
export function deviceDetail(d: DeviceInfo): string {
  const label = deviceLabel(d);
  return [d.model, d.serial, d.ip]
    .filter((part): part is string => !!part && part !== label)
    .join(" · ");
}
