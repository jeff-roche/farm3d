import { createSignal, Show } from "solid-js";
import { Button } from "../design-system";
import { isCommandError } from "../ipc/client";
import styles from "./StalePreparationBanner.module.css";

export interface StalePreparationBannerProps {
  /** The pinned source revision's number, if known. */
  pinnedSequence: number | undefined;
  /** The Model's current revision number. */
  currentSequence: number;
  /** **Continue with revision M** is chosen. */
  continuing: boolean;
  /** `reload_preparation`; rejects with the backend's error. */
  onReload: () => Promise<void>;
  onContinue: () => void;
  onWithdrawContinue: () => void;
}

/** D5/Errors and recovery: a stale Preparation (its source changed) offers
 *  **Reload onto revision N**, which re-bases it, or **Continue with
 *  revision M**, the deliberate choice `start_slice` needs to slice the
 *  pinned revision anyway. */
export function StalePreparationBanner(props: StalePreparationBannerProps) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | undefined>();
  const pinned = () => (props.pinnedSequence === undefined ? "an earlier revision" : `revision ${props.pinnedSequence}`);
  const pinnedShort = () => (props.pinnedSequence === undefined ? "the earlier revision" : `revision ${props.pinnedSequence}`);

  const reload = async () => {
    setBusy(true);
    setError(undefined);
    try {
      await props.onReload();
    } catch (failure) {
      setError(isCommandError(failure) ? failure.message : "The Preparation could not be reloaded.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class={styles.banner} role="region" aria-label="Source changed">
      <p class={styles.message}>
        <Show
          when={props.continuing}
          fallback={<>The Model changed: this Preparation uses {pinned()}, and the Model is now at revision {props.currentSequence}.</>}
        >
          Slicing will use {pinnedShort()}, as you chose. The Model is now at revision {props.currentSequence}.
        </Show>
      </p>
      <div class={styles.actions}>
        <Button variant="primary" size="sm" disabled={busy()} onClick={() => void reload()}>
          Reload onto revision {props.currentSequence}
        </Button>
        <Show
          when={props.continuing}
          fallback={
            <Button variant="secondary" size="sm" disabled={busy()} onClick={props.onContinue}>
              Continue with {pinnedShort()}
            </Button>
          }
        >
          <Button variant="ghost" size="sm" onClick={props.onWithdrawContinue}>Choose again</Button>
        </Show>
      </div>
      <Show when={error()}>{(message) => <p class={styles.error} role="alert">{message()}</p>}</Show>
    </div>
  );
}
