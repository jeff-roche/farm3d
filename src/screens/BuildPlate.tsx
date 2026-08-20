import styles from "./BuildPlate.module.css";

/**
 * A static perspective floor grid — the print bed a Model sits on inside
 * the Model Inspector viewport.
 */
export function BuildPlate() {
  return (
    <div class={styles.stage}>
      <div class={styles.plane} />
    </div>
  );
}
