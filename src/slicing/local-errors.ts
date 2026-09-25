/** `CommandError`s the slicing store makes itself, in the backend's shape:
 *  for what it refuses locally (an unknown id) and for web mode's local
 *  Preparation checks, which copy the backend's wording. */
import type { CommandError } from "../generated/contracts/command/CommandError";
import type { ErrorCode } from "../generated/contracts/command/ErrorCode";

export function commandError(code: ErrorCode, message: string, details?: Record<string, string>): CommandError {
  return {
    contractVersion: 1,
    code,
    message,
    recovery: code === "VALIDATION" ? ["EDIT_FIELDS"] : [],
    retryable: false,
    ...(details ? { details } : {}),
  };
}

export function notFound(id: string): CommandError {
  return commandError("NOT_FOUND", id, { entityId: id });
}

export function validationError(fieldPath: string, message: string): CommandError {
  return commandError("VALIDATION", message, { fieldPath });
}
