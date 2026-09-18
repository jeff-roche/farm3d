import { createSignal, type Accessor } from "solid-js";
import type { NavigationTarget as GeneratedNavigationTarget } from "../generated/contracts/navigation/NavigationTarget";

export type NavigationTarget = GeneratedNavigationTarget;
export type NavigationDestination = NavigationTarget["destination"];
export type NavigationAvailability = "available" | "destinationUnavailable" | "selectionUnavailable";

const allowed = {
  monitor: new Set(["printer", "incident", "attention"]),
  queue: new Set(["job"]),
  library: new Set(["model", "project"]),
  spools: new Set(["spool"]),
  settings: new Set<string>(),
} as const;

const unreserved = /^[A-Za-z0-9._~-]$/;

function encodeId(value: string): string {
  return Array.from(new TextEncoder().encode(value), (byte) => {
    const character = String.fromCharCode(byte);
    return unreserved.test(character) ? character : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
  }).join("");
}

function validId(kind: string, id: string): boolean {
  const bytes = new TextEncoder().encode(id).length;
  return bytes > 0 && bytes <= (kind === "printer" ? 512 : 256) && (kind !== "printer" || !/[\u0000-\u001F\u007F-\u009F]/u.test(id));
}

export function serializeNavigationTarget(target: NavigationTarget): string {
  if (!isValid(target)) throw new Error("Invalid navigation target");
  const base = `#nav=v1/${target.destination}`;
  return target.selection
    ? `${base}/${target.selection.kind}/${encodeId(target.selection.id)}`
    : base;
}

function isValid(target: NavigationTarget): boolean {
  if (target.version !== 1 || !(target.destination in allowed)) return false;
  if (!target.selection) return true;
  return allowed[target.destination].has(target.selection.kind) && validId(target.selection.kind, target.selection.id);
}

export function parseNavigationTarget(fragment: string): NavigationTarget | null {
  const parts = fragment.match(/^#nav=v1\/([^/]+)(?:\/([^/]+)\/([^/]+))?$/u);
  if (!parts) return null;
  const [, destination, kind, encodedId] = parts;
  let id: string | undefined;
  try {
    id = encodedId === undefined ? undefined : decodeURIComponent(encodedId);
  } catch {
    return null;
  }
  const target = {
    version: 1,
    destination,
    ...(kind && id !== undefined ? { selection: { kind, id } } : {}),
  } as NavigationTarget;
  return isValid(target) ? target : null;
}

export interface NavigationAvailabilityContext {
  availableDestinations: NavigationDestination[];
  availableIds: string[];
}

export function createNavigationStore(initial: NavigationTarget = { version: 1, destination: "monitor" }): {
  target: Accessor<NavigationTarget>;
  availability: Accessor<NavigationAvailability>;
  navigate: (target: NavigationTarget, context?: NavigationAvailabilityContext) => void;
} {
  const [target, setTarget] = createSignal(initial);
  const [availability, setAvailability] = createSignal<NavigationAvailability>("available");
  return {
    target,
    availability,
    navigate(next, context = { availableDestinations: ["monitor", "library"], availableIds: [] }) {
      if (!isValid(next)) throw new Error("Invalid navigation target");
      setTarget(next);
      if (!context.availableDestinations.includes(next.destination)) setAvailability("destinationUnavailable");
      else if (next.selection && !context.availableIds.includes(next.selection.id)) setAvailability("selectionUnavailable");
      else setAvailability("available");
    },
  };
}

export const navigation = createNavigationStore();
