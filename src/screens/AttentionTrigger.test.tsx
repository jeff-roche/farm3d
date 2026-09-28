import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  attentionStoreMock,
  resetAttentionStoreMock,
  setAttentionStoreState,
} from "../attention/attention-store-mock";
import { attentionEvent } from "../attention/test-records";
import { navigation } from "../navigation/navigation-store";
import { AttentionTrigger } from "./AttentionTrigger";

const { requestAttentionCenterOpen } = attentionStoreMock;

vi.mock("../attention/attention-store", async () => (await import("../attention/attention-store-mock")).attentionStoreMock);

afterEach(() => {
  cleanup();
  resetAttentionStoreMock();
  window.location.hash = "";
  // `navigation` is a module-level singleton; leaving it mutated would leak
  // into later tests in this file.
  navigation.navigate({ version: 1, destination: "monitor" });
});

describe("AttentionTrigger", () => {
  it("names itself 'nothing needs action' when nothing is open", () => {
    render(() => <AttentionTrigger />);
    expect(screen.getByRole("button", { name: "Attention: nothing needs action" })).toBeInTheDocument();
  });

  it("names itself with the actionable count and the highest open severity", () => {
    setAttentionStoreState({
      events: [
        attentionEvent({ id: "atn-1", severity: "warning", requiresAction: true, resolvedAt: null }),
        attentionEvent({ id: "atn-2", severity: "fatal", requiresAction: true, resolvedAt: null }),
      ],
    });
    render(() => <AttentionTrigger />);
    expect(screen.getByRole("button", { name: "Attention: 2 actionable, highest Fatal" })).toBeInTheDocument();
  });

  it("opens the popover with the keyboard (Enter/Space press a button through its click)", async () => {
    render(() => <AttentionTrigger />);
    const trigger = screen.getByRole("button", { name: "Attention: nothing needs action" });
    trigger.focus();
    await fireEvent.click(trigger);
    await waitFor(() => expect(screen.getByText("Nothing needs your attention.")).toBeInTheDocument());
  });

  it("selecting a row navigates to monitor/attention/<id> and closes the popover", async () => {
    setAttentionStoreState({
      events: [attentionEvent({ id: "atn-1", summary: "Bay 1 is offline.", requiresAction: true, resolvedAt: null })],
    });
    render(() => <AttentionTrigger />);
    const trigger = screen.getByRole("button", { name: /Attention:/ });
    await fireEvent.click(trigger);
    await screen.findByText("Bay 1 is offline.");

    await fireEvent.click(screen.getByText("Bay 1 is offline."));

    expect(window.location.hash).toBe("#nav=v1/monitor/attention/atn-1");
    await waitFor(() => expect(screen.queryByText("Bay 1 is offline.")).not.toBeInTheDocument());
  });

  it("keeps the filter selection across closing and reopening the popover", async () => {
    setAttentionStoreState({
      events: [attentionEvent({ id: "atn-1", resolvedAt: null, requiresAction: false })],
    });
    render(() => <AttentionTrigger />);
    const trigger = screen.getByRole("button", { name: /Attention:/ });

    await fireEvent.click(trigger);
    // Default filter is Actionable; this Event doesn't require action.
    await screen.findByText("No Events match this filter.");
    await fireEvent.click(screen.getByText("All open"));
    await screen.findByText(attentionEvent().summary);

    // Close (Escape) and reopen: the "All open" filter is still selected.
    await fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await waitFor(() => expect(screen.queryByText(attentionEvent().summary)).not.toBeInTheDocument());
    await fireEvent.click(trigger);
    await screen.findByText(attentionEvent().summary);
  });

  it("closes the popover on a navigation elsewhere (a pasted deep link, not a row click here)", async () => {
    setAttentionStoreState({
      events: [attentionEvent({ id: "atn-1", summary: "Bay 1 is offline.", requiresAction: true, resolvedAt: null })],
    });
    render(() => <AttentionTrigger />);
    const trigger = screen.getByRole("button", { name: /Attention:/ });
    await fireEvent.click(trigger);
    await screen.findByText("Bay 1 is offline.");

    // A hash-only change never remounts the SPA; simulated here the same
    // way App's own hashchange listener would drive it.
    navigation.navigate({ version: 1, destination: "monitor", selection: { kind: "attention", id: "atn-1" } });

    await waitFor(() => expect(screen.queryByText("Bay 1 is offline.")).not.toBeInTheDocument());
  });

  it("opens the center on requestAttentionCenterOpen (the notification-click seam)", async () => {
    render(() => <AttentionTrigger />);
    expect(screen.queryByText("Nothing needs your attention.")).not.toBeInTheDocument();

    requestAttentionCenterOpen();

    await waitFor(() => expect(screen.getByText("Nothing needs your attention.")).toBeInTheDocument());
  });

  it("announces a new live fatal or warning Event in its live region, not a backfilled one", async () => {
    render(() => <AttentionTrigger />);
    const live = document.querySelector('[aria-live="polite"]');
    expect(live?.textContent).toBe("");

    setAttentionStoreState({
      events: [attentionEvent({
        id: "atn-live", severity: "fatal", origin: "live", resolvedAt: null,
        summary: "Bay 1 printer-reported failure.",
      })],
    });

    await waitFor(() => expect(live?.textContent).toBe("New fatal Attention Event: Bay 1 printer-reported failure."));
  });
});
