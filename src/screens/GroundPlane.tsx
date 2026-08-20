import styles from "./GroundPlane.module.css";

/**
 * A static perspective floor grid — the shared signature between the welcome
 * screen and the editor viewport. Both represent the same "plot of land" a
 * farm3d project starts from, so they share the exact same backdrop.
 */
export function GroundPlane() {
  return (
    <div class={styles.stage}>
      <div class={styles.plane} />
    </div>
  );
}
