import { createSignal } from "solid-js";
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DataTable, type DataTableColumn } from "./DataTable";

afterEach(() => {
  document.body.innerHTML = "";
});

interface Row {
  id: string;
  name: string;
  amount: number;
}

const rows: Row[] = [
  { id: "a", name: "Alpha", amount: 3 },
  { id: "b", name: "Bravo", amount: 1 },
  { id: "c", name: "Charlie", amount: 2 },
];

const columns: DataTableColumn<Row>[] = [
  { id: "name", header: "Name", cell: (row) => row.name, sortValue: (row) => row.name },
  { id: "amount", header: "Amount", cell: (row) => String(row.amount), sortValue: (row) => row.amount, align: "end" },
];

function SelectableTable(props: { onActivate?: (id: string) => void }) {
  const [selectedId, setSelectedId] = createSignal<string | null>("a");
  return (
    <DataTable
      label="Rows"
      rows={rows}
      rowId={(row) => row.id}
      columns={columns}
      selectedId={selectedId()}
      onSelect={setSelectedId}
      onActivate={props.onActivate}
    />
  );
}

describe("DataTable", () => {
  it("renders role=grid with aria-rowcount", () => {
    render(() => (
      <DataTable label="Rows" rows={rows} rowId={(row) => row.id} columns={columns} />
    ));

    const grid = screen.getByRole("grid", { name: "Rows" });
    expect(grid.getAttribute("aria-rowcount")).toBe("3");
  });

  it("moves selection with ArrowDown/ArrowUp/Home/End and calls onActivate on Enter", async () => {
    const onActivate = vi.fn();
    render(() => <SelectableTable onActivate={onActivate} />);

    const grid = screen.getByRole("grid", { name: "Rows" });

    expect(screen.getByRole("row", { name: /Alpha/ }).getAttribute("aria-selected")).toBe("true");

    await fireEvent.keyDown(grid, { key: "ArrowDown" });
    expect(screen.getByRole("row", { name: /Bravo/ }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("row", { name: /Alpha/ }).getAttribute("aria-selected")).toBe("false");

    await fireEvent.keyDown(grid, { key: "End" });
    expect(screen.getByRole("row", { name: /Charlie/ }).getAttribute("aria-selected")).toBe("true");

    await fireEvent.keyDown(grid, { key: "Home" });
    expect(screen.getByRole("row", { name: /Alpha/ }).getAttribute("aria-selected")).toBe("true");

    await fireEvent.keyDown(grid, { key: "ArrowUp" });
    expect(screen.getByRole("row", { name: /Alpha/ }).getAttribute("aria-selected")).toBe("true");

    await fireEvent.keyDown(grid, { key: "ArrowDown" });
    await fireEvent.keyDown(grid, { key: "Enter" });
    expect(onActivate).toHaveBeenCalledWith("b");
  });

  it("toggles aria-sort on header click", async () => {
    const onSortChange = vi.fn();
    render(() => (
      <DataTable
        label="Rows"
        rows={rows}
        rowId={(row) => row.id}
        columns={columns}
        onSortChange={onSortChange}
      />
    ));

    const nameHeader = screen.getByRole("columnheader", { name: "Name" });
    expect(nameHeader.getAttribute("aria-sort")).toBe("none");

    await fireEvent.click(screen.getByText("Name"));
    expect(onSortChange).toHaveBeenCalledWith({ columnId: "name", direction: "asc" });
  });

  it("reflects a controlled sort direction and toggles it", async () => {
    const onSortChange = vi.fn();
    render(() => (
      <DataTable
        label="Rows"
        rows={rows}
        rowId={(row) => row.id}
        columns={columns}
        sort={{ columnId: "name", direction: "asc" }}
        onSortChange={onSortChange}
      />
    ));

    const nameHeader = screen.getByRole("columnheader", { name: "Name" });
    expect(nameHeader.getAttribute("aria-sort")).toBe("ascending");

    await fireEvent.click(screen.getByText("Name"));
    expect(onSortChange).toHaveBeenCalledWith({ columnId: "name", direction: "desc" });
  });

  it("shows the empty state when there are no rows", () => {
    render(() => (
      <DataTable
        label="Rows"
        rows={[]}
        rowId={(row: Row) => row.id}
        columns={columns}
        empty={<span>No rows yet</span>}
      />
    ));

    expect(screen.getByText("No rows yet")).toBeInTheDocument();
  });
});
