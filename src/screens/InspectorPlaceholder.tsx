import type { ParentProps } from "solid-js";
import styles from "./InspectorPlaceholder.module.css";

/** Holds the Model details 3D inspector's place while it (or its data)
 *  loads, or says why it can't show, at the viewport's own height, so
 *  the panel doesn't jump. Kept out of the lazily loaded inspector so
 *  the Suspense fallback can use it before that chunk arrives. */
export function InspectorPlaceholder(props: ParentProps) {
  return <p class={styles.placeholder} role="status">{props.children}</p>;
}
