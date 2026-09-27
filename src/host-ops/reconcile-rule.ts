/** P6's enabling rules for **Check again** and **Abandon check…** on an
 *  uncertain Host Operation (D5 "Capability gate", D8), shared by the
 *  Printer's Job tab and P7's `JobPanel` so the two never disagree. Pure:
 *  nothing here calls a command. */
import type { CapabilityKey, HostOperation, PrinterCapabilities } from "./types";

/** The capability a row's reconciliation needs. */
export function reconcileCapability(operation: HostOperation): CapabilityKey {
  return operation.kind === "upload" ? "artifactIdentity" : "hostState";
}

export function supports(record: PrinterCapabilities | undefined, key: CapabilityKey): boolean {
  return record?.capabilities[key].status === "supported";
}

/** **Check again** is enabled when the Printer supports the check. */
export function canReconcile(operation: HostOperation, record: PrinterCapabilities | undefined): boolean {
  return supports(record, reconcileCapability(operation));
}

/** **Abandon check…** is enabled for an uncertain row once farm3d has
 *  checked at least once, or when it can't check at all. */
export function canAbandon(operation: HostOperation, record: PrinterCapabilities | undefined): boolean {
  return operation.state === "uncertain" && (operation.attempts >= 1 || !canReconcile(operation, record));
}
