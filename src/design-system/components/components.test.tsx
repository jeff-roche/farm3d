import { createSignal } from "solid-js";
import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Button } from "./Button";
import { Checkbox } from "./Checkbox";
import { Switch } from "./Switch";
import { TextField } from "./TextField";
import { Select } from "./Select";
import { Tabs } from "./Tabs";
import { Dialog } from "./Dialog";
import { Popover } from "./Popover";
import { DropdownMenu } from "./DropdownMenu";
import { Chip } from "./Chip";
import { Logo } from "./Logo";
import { NumberField } from "./NumberField";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("Button", () => {
  it("fires onClick and respects disabled", async () => {
    const onClick = vi.fn();
    render(() => (
      <>
        <Button onClick={onClick}>Click me</Button>
        <Button onClick={onClick} disabled>
          Disabled
        </Button>
      </>
    ));

    await fireEvent.click(screen.getByText("Click me"));
    expect(onClick).toHaveBeenCalledTimes(1);

    await fireEvent.click(screen.getByText("Disabled"));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});

describe("Checkbox", () => {
  it("reflects and updates controlled checked state", async () => {
    function Harness() {
      const [checked, setChecked] = createSignal(false);
      return (
        <Checkbox checked={checked()} onChange={setChecked}>
          Agree
        </Checkbox>
      );
    }
    render(() => <Harness />);

    const input = screen.getByRole("checkbox") as HTMLInputElement;
    expect(input.checked).toBe(false);

    await fireEvent.click(input);
    expect(input.checked).toBe(true);
  });
});

describe("Switch", () => {
  it("toggles on click", async () => {
    const onChange = vi.fn();
    render(() => <Switch onChange={onChange}>Enabled</Switch>);

    const input = screen.getByRole("switch") as HTMLInputElement;
    await fireEvent.click(input);
    expect(onChange).toHaveBeenCalledWith(true);
  });
});

describe("TextField", () => {
  it("calls onChange as the user types", async () => {
    const onChange = vi.fn();
    render(() => <TextField label="Name" onChange={onChange} />);

    const input = screen.getByLabelText("Name") as HTMLInputElement;
    await fireEvent.input(input, { target: { value: "farm3d" } });
    expect(onChange).toHaveBeenCalledWith("farm3d");
  });

  it("shows the error message when provided", () => {
    render(() => <TextField label="Name" error="Required" />);
    expect(screen.getByText("Required")).toBeInTheDocument();
  });
});

describe("Chip", () => {
  it("toggles selected state and fires onRemove independently", async () => {
    const onSelectedChange = vi.fn();
    const onRemove = vi.fn();
    render(() => (
      <Chip onSelectedChange={onSelectedChange} onRemove={onRemove}>
        Tag
      </Chip>
    ));

    await fireEvent.click(screen.getByText("Tag"));
    expect(onSelectedChange).toHaveBeenCalledWith(true);

    await fireEvent.click(screen.getByLabelText("Remove"));
    expect(onRemove).toHaveBeenCalledTimes(1);
  });
});

describe("Select", () => {
  it("opens the listbox and selects an option", async () => {
    const onChange = vi.fn();
    render(() => (
      <Select label="Fruit" options={["Apple", "Banana"]} onChange={onChange} />
    ));

    await fireEvent.pointerDown(screen.getByRole("button"), { pointerType: "mouse", button: 0 });
    const option = await screen.findByText("Banana");
    await fireEvent.click(option);

    expect(onChange).toHaveBeenCalledWith("Banana");
  });
});

describe("Tabs", () => {
  it("switches visible content when a trigger is clicked", async () => {
    render(() => (
      <Tabs
        items={[
          { value: "a", label: "Tab A", content: "Content A" },
          { value: "b", label: "Tab B", content: "Content B" },
        ]}
        defaultValue="a"
      />
    ));

    expect(screen.getByText("Content A")).toBeInTheDocument();
    await fireEvent.click(screen.getByText("Tab B"));
    await waitFor(() => expect(screen.getByText("Content B")).toBeInTheDocument());
  });
});

describe("Dialog", () => {
  it("opens on trigger click and shows its title", async () => {
    render(() => (
      <Dialog title="Confirm" trigger="Open">
        Body content
      </Dialog>
    ));

    expect(screen.queryByText("Confirm")).not.toBeInTheDocument();
    await fireEvent.click(screen.getByText("Open"));
    await waitFor(() => expect(screen.getByText("Confirm")).toBeInTheDocument());
  });
});

describe("Popover", () => {
  it("opens on trigger click and shows its content", async () => {
    render(() => <Popover trigger="Open">Popover content</Popover>);

    expect(screen.queryByText("Popover content")).not.toBeInTheDocument();
    await fireEvent.click(screen.getByText("Open"));
    await waitFor(() => expect(screen.getByText("Popover content")).toBeInTheDocument());
  });

  it("shows content when externally controlled open is true", async () => {
    function Harness() {
      const [open, setOpen] = createSignal(false);
      return (
        <>
          <button onClick={() => setOpen(true)}>External open</button>
          <Popover open={open()} onOpenChange={setOpen}>
            Popover content
          </Popover>
        </>
      );
    }
    render(() => <Harness />);

    expect(screen.queryByText("Popover content")).not.toBeInTheDocument();
    await fireEvent.click(screen.getByText("External open"));
    await waitFor(() => expect(screen.getByText("Popover content")).toBeInTheDocument());
  });
});

describe("Logo", () => {
  it("defaults to a 24px svg", () => {
    const { container } = render(() => <Logo />);
    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("width")).toBe("24");
    expect(svg?.getAttribute("height")).toBe("24");
  });

  it("respects the size prop", () => {
    const { container } = render(() => <Logo size={48} />);
    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("width")).toBe("48");
  });

  it("gives each instance a unique clip-path id", () => {
    const { container } = render(() => (
      <>
        <Logo />
        <Logo />
      </>
    ));
    const ids = [...container.querySelectorAll("clipPath")].map((el) => el.id);
    expect(new Set(ids).size).toBe(2);
  });
});

describe("NumberField", () => {
  it("calls onChange with the incremented raw value when the increment trigger is clicked", async () => {
    const onChange = vi.fn();
    render(() => <NumberField label="Height" value={10} step={1} onChange={onChange} />);
    const increment = screen.getByLabelText("Increment");
    await fireEvent.click(increment);
    expect(onChange).toHaveBeenCalledWith(11);
  });

  it("does not exceed maxValue", async () => {
    const onChange = vi.fn();
    render(() => (
      <NumberField label="Height" value={10} maxValue={10} step={1} onChange={onChange} />
    ));
    const increment = screen.getByLabelText("Increment");
    await fireEvent.click(increment);
    expect(onChange).not.toHaveBeenCalledWith(11);
  });

  it("renders the suffix text", () => {
    render(() => <NumberField label="Height" value={10} suffix="mm" />);
    expect(screen.getByText("mm")).toBeInTheDocument();
  });
});

describe("DropdownMenu", () => {
  it("opens and invokes onSelect for the clicked item", async () => {
    const onSelect = vi.fn();
    render(() => (
      <DropdownMenu
        trigger="Actions"
        items={[
          { label: "Rename", onSelect },
          { type: "separator" },
          { label: "Delete", onSelect: vi.fn() },
        ]}
      />
    ));

    await fireEvent.pointerDown(screen.getByText("Actions"), { pointerType: "mouse", button: 0 });
    const item = await screen.findByText("Rename");
    await fireEvent.pointerUp(item, { button: 0 });

    expect(onSelect).toHaveBeenCalledTimes(1);
  });
});
