import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setAttentionStoreState, resetAttentionStoreMock } from "../attention/attention-store-mock";
import { attentionEvent } from "../attention/test-records";
import { DEFAULT_ATTENTION_FILTER, type AttentionFilter, type AttentionSeverityFilter } from "../attention/presentation";
import { AttentionCenter } from "./AttentionCenter";

vi.mock("../attention/attention-store", async () => (await import("../attention/attention-store-mock")).attentionStoreMock);

afterEach(() => {
  cleanup();
  resetAttentionStoreMock();
});

function harness(overrides: Partial<{ filter: AttentionFilter; severityFilter: AttentionSeverityFilter }> = {}) {
  const onFilterChange = vi.fn();
  const onSeverityFilterChange = vi.fn();
  const onSelect = vi.fn();
  render(() => (
    <AttentionCenter
      filter={overrides.filter ?? DEFAULT_ATTENTION_FILTER}
      onFilterChange={onFilterChange}
      severityFilter={overrides.severityFilter ?? "all"}
      onSeverityFilterChange={onSeverityFilterChange}
      onSelect={onSelect}
    />
  ));
  return { onFilterChange, onSeverityFilterChange, onSelect };
}

describe("AttentionCenter", () => {
  it("shows the distinct empty state when nothing is open at all", () => {
    setAttentionStoreState({ events: [] });
    harness();
    expect(screen.getByText("Nothing needs your attention.")).toBeInTheDocument();
  });

  it("shows the distinct filtered-empty state when Events exist but none match the filter", () => {
    setAttentionStoreState({
      events: [attentionEvent({ id: "atn-1", requiresAction: false, resolvedAt: null })],
    });
    harness({ filter: "actionable" });
    expect(screen.getByText("No Events match this filter.")).toBeInTheDocument();
    expect(screen.queryByText("Nothing needs your attention.")).not.toBeInTheDocument();
  });

  it("lists open Events with severity, summary, and selecting a row calls onSelect", async () => {
    const event = attentionEvent({
      id: "atn-1", requiresAction: true, resolvedAt: null, severity: "fatal", summary: "Bay 1 printer-reported failure.",
    });
    setAttentionStoreState({ events: [event] });
    const { onSelect } = harness({ filter: "actionable" });

    expect(screen.getByText("Bay 1 printer-reported failure.")).toBeInTheDocument();
    expect(screen.getByRole("status", { name: "Fatal" })).toBeInTheDocument();

    await fireEvent.click(screen.getByText("Bay 1 printer-reported failure."));
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ id: "atn-1" }));
  });

  it("changing the filter calls onFilterChange rather than filtering internally", async () => {
    setAttentionStoreState({ events: [] });
    const { onFilterChange } = harness();

    await fireEvent.click(screen.getByText("Unread"));
    expect(onFilterChange).toHaveBeenCalledWith("unread");
  });

  it("changing the severity Select calls onSeverityFilterChange", async () => {
    setAttentionStoreState({
      events: [attentionEvent({ id: "atn-1", requiresAction: true, resolvedAt: null, severity: "fatal" })],
    });
    const { onSeverityFilterChange } = harness({ filter: "actionable" });

    await fireEvent.pointerDown(screen.getByRole("button", { name: /All severities/i }), { pointerType: "mouse", button: 0 });
    const option = await screen.findByText("Fatal", { selector: '[id^="select-"], li, [role="option"]' });
    await fireEvent.click(option);

    expect(onSeverityFilterChange).toHaveBeenCalledWith("fatal");
  });

  it("keeps its filter selection controlled by props, not internal state", () => {
    setAttentionStoreState({ events: [] });
    render(() => (
      <AttentionCenter
        filter="resolved"
        onFilterChange={vi.fn()}
        severityFilter="all"
        onSeverityFilterChange={vi.fn()}
        onSelect={vi.fn()}
      />
    ));
    // Nothing resolved is loaded, so the empty state reflects the Resolved scope.
    expect(screen.getByText("Nothing needs your attention.")).toBeInTheDocument();
  });
});
