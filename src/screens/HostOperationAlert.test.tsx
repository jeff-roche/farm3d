import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetQueueStoreMock, setQueueStoreState } from "../queue/queue-store-mock";
import { job } from "../queue/test-records";
import { HostOperationAlert } from "./HostOperationAlert";

vi.mock("../queue/queue-store", async () => (await import("../queue/queue-store-mock")).queueStoreMock);
vi.mock("../host-ops/host-operations-store", async () =>
  (await import("../host-ops/host-operations-store-mock")).hostOperationsStoreMock);
vi.mock("../printers/printer-store", () => ({ printers: () => [] }));

beforeEach(() => {
  resetQueueStoreMock();
  window.location.hash = "";
});
afterEach(cleanup);

describe("HostOperationAlert: OPEN_JOB", () => {
  it("opens the Job a Job-linked CONNECTION_IN_USE names", () => {
    const onOpenJob = vi.fn();
    render(() => (
      <HostOperationAlert
        error={{
          contractVersion: 1, code: "CONNECTION_IN_USE",
          message: "This Printer's Job is waiting on a printer operation. Let it finish, or abandon the check from the Job, before changing this Connection.",
          details: { printerId: "prn-1", hostOperationId: "hop-1", jobId: "job-7" }, recovery: ["OPEN_JOB"], retryable: false,
        }}
        fallback="Couldn't save."
        onOpenJob={onOpenJob}
      />
    ));
    expect(screen.getByRole("alert")).toHaveTextContent("This Printer's Job is waiting on a printer operation.");
    fireEvent.click(screen.getByRole("button", { name: "Open the Job" }));
    expect(window.location.hash).toBe("#nav=v1/queue/job/job-7");
    expect(onOpenJob).toHaveBeenCalled();
  });

  it("falls back to the Printer's active Job for JOB_ACTIVE without a jobId", () => {
    setQueueStoreState({ jobs: [job({ id: "job-3", printerId: "prn-2", state: "printing" })] });
    render(() => (
      <HostOperationAlert
        error={{
          contractVersion: 1, code: "JOB_ACTIVE", message: "This Printer has an active Job. Use the Job's controls.",
          details: { printerId: "prn-2" }, recovery: ["OPEN_JOB"], retryable: false,
        }}
        fallback="Couldn't start."
      />
    ));
    fireEvent.click(screen.getByRole("button", { name: "Open the Job" }));
    expect(window.location.hash).toBe("#nav=v1/queue/job/job-3");
  });
});
