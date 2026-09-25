/** What `PlateViewport` shows, and its text description (the
 *  Accessibility section): the plate, the object count, the selected
 *  object's position, rotation and scale, the objects outside the
 *  printable area, and any caller notes. */
import type { InstanceTransform } from "../types";

export interface ViewportInstance {
  instanceKey: string;
  objectKey: number;
  /** The object's name, as the description and object list show it. */
  name: string;
  transform: InstanceTransform;
  /** Tinted, and listed in the description. */
  outOfBounds?: boolean;
}

export interface PlateDescriptionInput {
  plateName: string;
  instances: ViewportInstance[];
  selectedInstanceKey?: string | null;
  /** Further sentences, e.g. the items that were not placed. */
  notes?: string[];
}

const mm = (value: number) => `${value.toFixed(1)}`;
const degrees = (value: number) => `${Number(value.toFixed(1))}°`;
const percent = (value: number) => `${Number((value * 100).toFixed(1))}%`;

function list(names: string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

export function describePlate(input: PlateDescriptionInput): string {
  const count = input.instances.length;
  const sentences = [`Plate ${input.plateName}: ${count === 1 ? "1 object" : `${count} objects`}.`];
  const selected = input.instances.find((instance) => instance.instanceKey === input.selectedInstanceKey);
  if (selected) {
    const { translateMm: [x, y], rotateDeg, scale } = selected.transform;
    sentences.push(
      `Selected: ${selected.name}, at X ${mm(x)} mm, Y ${mm(y)} mm;`
        + ` rotated ${rotateDeg.map(degrees).join(", ")} about X, Y, Z;`
        + ` scaled ${scale.map(percent).join(", ")}.`,
    );
  } else if (count > 0) {
    sentences.push("No object selected.");
  }
  const outside = input.instances.filter((instance) => instance.outOfBounds).map((instance) => instance.name);
  if (outside.length > 0) sentences.push(`Outside the printable area: ${list(outside)}.`);
  return [...sentences, ...(input.notes ?? [])].join(" ");
}
