import { createSignal } from "solid-js";
import { Portal } from "solid-js/web";
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

function SelectableTable(props: {
  onActivate?: (id: string) => void;
  onSortChange?: (sort: { columnId: string; direction: "asc" | "desc" }) => void;
}) {
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
      onSortChange={props.onSortChange}
    />
  );
}

describe("DataTable", () => {
  it("renders role=grid with aria-rowcount counting the header row", () => {
    render(() => (
      <DataTable label="Rows" rows={rows} rowId={(row) => row.id} columns={columns} />
    ));

    const grid = screen.getByRole("grid", { name: "Rows" });
    // Three data rows plus the header row.
    expect(grid.getAttribute("aria-rowcount")).toBe("4");
  });

  it("gives the header row aria-rowindex 1 and data rows 2 onward", () => {
    render(() => (
      <DataTable label="Rows" rows={rows} rowId={(row) => row.id} columns={columns} />
    ));

    const allRows = screen.getAllByRole("row");
    expect(allRows.map((row) => row.getAttribute("aria-rowindex"))).toEqual(["1", "2", "3", "4"]);
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

  it("scopes keyboard handling to rows: header Enter sorts without activating, header ArrowDown leaves selection alone", async () => {
    const onActivate = vi.fn();
    const onSortChange = vi.fn();
    render(() => <SelectableTable onActivate={onActivate} onSortChange={onSortChange} />);

    const nameHeaderButton = screen.getByText("Name");
    nameHeaderButton.focus();

    // A real browser fires both `keydown` and, as the focused <button>'s
    // native default action, a `click` when Enter is pressed. jsdom's
    // `fireEvent.keyDown` doesn't synthesize that click for us, so this
    // fires both to reproduce what a keyboard user actually triggers.
    await fireEvent.keyDown(nameHeaderButton, { key: "Enter" });
    await fireEvent.click(nameHeaderButton);
    expect(onSortChange).toHaveBeenCalledWith({ columnId: "name", direction: "asc" });
    expect(onActivate).not.toHaveBeenCalled();

    await fireEvent.keyDown(nameHeaderButton, { key: "ArrowDown" });
    expect(screen.getByRole("row", { name: /Alpha/ }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("row", { name: /Bravo/ }).getAttribute("aria-selected")).toBe("false");
  });

  it("handles keys only on the table or its own rows: controls in a cell and portaled menus keep theirs", async () => {
    const onActivate = vi.fn();
    const onMenu = vi.fn();
    function TableWithControls() {
      const [selectedId, setSelectedId] = createSignal<string | null>("a");
      return (
        <DataTable
          label="Rows"
          rows={rows}
          rowId={(row) => row.id}
          columns={[
            ...columns,
            {
              id: "actions",
              header: "Actions",
              cell: (row) => (
                <>
                  <button type="button" onClick={onMenu}>Edit {row.name}</button>
                  <span role="button" tabIndex={0} aria-haspopup="menu">Menu {row.name}</span>
                  <input aria-label={`Note ${row.name}`} />
                  <div role="menu" tabIndex={-1} aria-label={`Inline menu ${row.name}`} />
                  <Portal>
                    <div role="menu" tabIndex={-1} aria-label={`Portaled menu ${row.name}`} />
                  </Portal>
                </>
              ),
            },
          ]}
          selectedId={selectedId()}
          onSelect={setSelectedId}
          onActivate={onActivate}
        />
      );
    }
    render(() => <TableWithControls />);
    const alpha = () => screen.getByRole("row", { name: /Alpha/ });

    for (const control of [
      screen.getByRole("button", { name: "Edit Alpha" }),
      screen.getByRole("button", { name: "Menu Alpha" }),
      screen.getByRole("textbox", { name: "Note Alpha" }),
      screen.getByRole("menu", { name: "Inline menu Alpha" }),
      // Opened menu content lives in a portal, but Solid's delegated
      // keydown still bubbles from it to the table.
      screen.getByRole("menu", { name: "Portaled menu Alpha" }),
    ]) {
      await fireEvent.keyDown(control, { key: "Enter" });
      await fireEvent.keyDown(control, { key: "ArrowDown" });
      await fireEvent.keyDown(control, { key: "End" });
    }
    expect(onActivate).not.toHaveBeenCalled();
    expect(alpha().getAttribute("aria-selected")).toBe("true");

    await fireEvent.keyDown(alpha(), { key: "Enter" });
    expect(onActivate).toHaveBeenCalledWith("a");
    await fireEvent.keyDown(alpha(), { key: "ArrowDown" });
    expect(screen.getByRole("row", { name: /Bravo/ }).getAttribute("aria-selected")).toBe("true");
  });

  it("is itself tabbable when nothing is selected, and selects+focuses the first row on ArrowDown", async () => {
    function UnselectedTable() {
      const [selectedId, setSelectedId] = createSignal<string | null>(null);
      return (
        <DataTable
          label="Rows"
          rows={rows}
          rowId={(row) => row.id}
          columns={columns}
          selectedId={selectedId()}
          onSelect={setSelectedId}
        />
      );
    }
    render(() => <UnselectedTable />);

    const grid = screen.getByRole("grid", { name: "Rows" });
    expect(grid.tabIndex).toBe(0);

    await fireEvent.keyDown(grid, { key: "ArrowDown" });

    const alphaRow = screen.getByRole("row", { name: /Alpha/ });
    expect(alphaRow.getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(alphaRow);
    expect(grid.tabIndex).toBe(-1);
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
