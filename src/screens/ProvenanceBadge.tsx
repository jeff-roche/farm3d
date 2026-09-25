import { IconAdjustments, IconCircleDashed, IconUserCheck } from "@tabler/icons-solidjs";
import type { Component } from "solid-js";
import { Dynamic } from "solid-js/web";
import { PROVENANCE_LABELS } from "../slicing/revision-presentation";
import type { FactProvenance } from "../slicing/types";
import styles from "./ProvenanceBadge.module.css";

const ICONS: Record<FactProvenance, Component<{ size?: number; "aria-hidden"?: "true" }>> = {
  farm3dInput: IconAdjustments,
  operatorConfirmed: IconUserCheck,
  absent: IconCircleDashed,
};

/** D15/D21: where a fact came from, told by text, an icon, the badge's
 *  shape (square, pill, dashed pill) and its colour, so no one of them is
 *  needed on its own. */
export function ProvenanceBadge(props: { provenance: FactProvenance }) {
  return (
    <span class={[styles.badge, styles[props.provenance]].join(" ")} data-provenance={props.provenance}>
      <Dynamic component={ICONS[props.provenance]} size={12} aria-hidden="true" />
      <span>{PROVENANCE_LABELS[props.provenance]}</span>
    </span>
  );
}
