import { createSignal } from "solid-js";
import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Button } from "./Button";
import { Checkbox } from "./Checkbox";
import { Switch } from "./Switch";
import { TextField } from "./TextField";
import { Select } from "./Select";
import { Combobox } from "./Combobox";
import { Tabs } from "./Tabs";
import { Dialog } from "./Dialog";
import { Popover } from "./Popover";
import { DropdownMenu } from "./DropdownMenu";
import { Chip } from "./Chip";
import { Logo } from "./Logo";
import { NumberField } from "./NumberField";
import { Field } from "./Field";

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

  it("forwards aria-label to the input itself, for an unlabeled field with no visible label", () => {
    // Mirrors NumberField's identical aria-label handling: Kobalte's
    // form-control primitives read aria-label from the Input subcomponent
    // specifically, not the Root, so it must be forwarded there rather
    // than spread onto the root element.
    render(() => <TextField aria-label="Printer name" onChange={vi.fn()} />);
    expect(screen.getByLabelText("Printer name") as HTMLInputElement).toHaveProperty(
      "tagName",
      "INPUT",
    );
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

describe("Combobox", () => {
  it("filters options via onInputChange and selects one on pointerup", async () => {
    const onChange = vi.fn();
    const onInputChange = vi.fn();
    render(() => (
      <Combobox
        label="Model"
        options={["Elegoo Centauri Carbon", "Prusa MK4", "Voron 2.4"]}
        onChange={onChange}
        onInputChange={onInputChange}
      />
    ));
    const input = screen.getByRole("combobox", { name: "Model" }) as HTMLInputElement;
    await fireEvent.pointerDown(input, { pointerType: "mouse", button: 0 });
    await fireEvent.input(input, { target: { value: "Prusa" } });
    expect(onInputChange).toHaveBeenCalledWith("Prusa");

    const item = await screen.findByText("Prusa MK4");
    await fireEvent.pointerUp(item, { pointerType: "mouse", button: 0 });
    expect(onChange).toHaveBeenCalledWith("Prusa MK4");
  });

  it("renders grouped options under section headers", async () => {
    render(() => (
      <Combobox
        label="Model"
        groups={[
          { label: "Elegoo", options: ["Centauri Carbon", "Neptune 4"] },
          { label: "Prusa", options: ["MK4", "CORE One"] },
        ]}
      />
    ));
    const trigger = screen.getByRole("button", { name: /show suggestions/i });
    await fireEvent.pointerDown(trigger, { pointerType: "mouse", button: 0 });
    expect(await screen.findByText("Elegoo")).toBeInTheDocument();
    expect(await screen.findByText("Prusa")).toBeInTheDocument();
  });

  it("suppresses an option's mousedown default so it never steals focus from the input", async () => {
    // Kobalte selects on pointerup, not mousedown — but an unprevented
    // mousedown still moves native focus to the item, which blurs the
    // input and resets the typed filter before the pointerup lands. That
    // re-expands the option list out from under the click, so it resolves
    // against whatever's now at that position instead of the clicked
    // option (reported: searching "Snapmaker" and clicking it selected
    // "Afinia", the first item in the unfiltered list). This can't be
    // reproduced in jsdom, which doesn't simulate mousedown's native
    // focus-shift — so this test pins the guard itself.
    render(() => (
      <Combobox label="Model" options={["Elegoo Centauri Carbon", "Prusa MK4", "Voron 2.4"]} />
    ));
    const input = screen.getByRole("combobox", { name: "Model" }) as HTMLInputElement;
    await fireEvent.pointerDown(input, { pointerType: "mouse", button: 0 });
    await fireEvent.input(input, { target: { value: "Prusa" } });

    const item = await screen.findByText("Prusa MK4");
    const event = new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 });
    item.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
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

describe("Field", () => {
  it("renders no revert control when not overridden", () => {
    render(() => <Field label="Printable height">240</Field>);
    expect(screen.queryByLabelText(/Revert/)).not.toBeInTheDocument();
  });

  it("renders a revert control when overridden and calls onRevert when clicked", async () => {
    const onRevert = vi.fn();
    render(() => (
      <Field label="Printable height" overridden onRevert={onRevert}>
        240
      </Field>
    ));
    const revert = screen.getByLabelText("Revert Printable height to inherited");
    await fireEvent.click(revert);
    expect(onRevert).toHaveBeenCalledTimes(1);
  });

  it("shows the hint text when not overridden", () => {
    render(() => (
      <Field label="Printable height" hint="inherited: 256">
        240
      </Field>
    ));
    expect(screen.getByText("inherited: 256")).toBeInTheDocument();
  });

  it("shows the hint text even when overridden (regression: hint should not gate on !overridden)", () => {
    render(() => (
      <Field label="Printable height" overridden hint="inherited: 256" onRevert={() => {}}>
        240
      </Field>
    ));
    expect(screen.getByText("inherited: 256")).toBeInTheDocument();
  });
});
