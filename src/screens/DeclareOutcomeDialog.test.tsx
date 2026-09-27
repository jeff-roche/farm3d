import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { queueStoreMock, resetQueueStoreMock } from "../queue/queue-store-mock";
import { job } from "../queue/test-records";
import { pressEscape, tabOrder } from "./dialog-test-helpers";
import { DeclareOutcomeDialog } from "./DeclareOutcomeDialog";

vi.mock("../queue/queue-store", async () => (await import("../queue/queue-store-mock")).queueStoreMock);

beforeEach(() => resetQueueStoreMock());
afterEach(cleanup);

const UNKNOWN = job({ state: "outcomeUnknown", allowedActions: ["declareOutcome"] });

describe("DeclareOutcomeDialog", () => {
  it("needs an outcome and the acknowledgement before it declares", async () => {
    render(() => <DeclareOutcomeDialog job={UNKNOWN} open onOpenChange={vi.fn()} />);
    const dialog = await screen.findByRole("alertdialog", { name: "Declare how this Job ended?" });
    const confirm = within(dialog).getByRole("button", { name: "Declare" });
    expect(confirm).toBeDisabled();
    fireEvent.click(within(dialog).getByLabelText("Failed"));
    expect(confirm).toBeDisabled();
    expect(within(dialog).getByText("Tick the acknowledgement to declare.")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("checkbox"));
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(confirm);
    await waitFor(() => expect(queueStoreMock.declareJobOutcome).toHaveBeenCalledWith("job-1", "failed"));
  });

  it("is keyboard operable: Tab walks the outcome, the acknowledgement, then the buttons; Escape closes", async () => {
    const onOpenChange = vi.fn();
    render(() => <DeclareOutcomeDialog job={UNKNOWN} open onOpenChange={onOpenChange} />);
    const dialog = await screen.findByRole("alertdialog", { name: "Declare how this Job ended?" });
    expect(tabOrder(dialog)).toEqual(["Completed", expect.stringMatching(/can't confirm/), "Not now"]);
    pressEscape(dialog);
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(queueStoreMock.declareJobOutcome).not.toHaveBeenCalled();
  });
});
