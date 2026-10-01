import { isCommandError } from "../../ipc/client";

/** A CommandError's message is written for the operator; anything else may
 *  carry internals, so it gets a generic line. */
export function errorText(error: unknown, fallback = "Something went wrong. Try again."): string {
  return isCommandError(error) ? error.message : fallback;
}
