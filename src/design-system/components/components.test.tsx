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
import { SeverityMarker } from "./SeverityMarker";
import { PrinterRoster } from "./PrinterRoster";
import { Stepper, type StepperStep } from "./Stepper";
import { Textarea } from "./Textarea";
import { FileDropSurface } from "./FileDropSurface";
import { SegmentedControl } from "./SegmentedControl";
import { Progress } from "./Progress";

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

    await fireEvent.click(screen.getByRole("button", { name: "Remove Tag" }));
    expect(onRemove).toHaveBeenCalledTimes(1);
    expect(onSelectedChange).toHaveBeenCalledTimes(1);
  });

  it("makes the remove control a native button in the tab order, labelled with the chip's text", () => {
    render(() => (
      <>
        <Chip onRemove={() => {}}>Brackets</Chip>
        <Chip onRemove={() => {}}>Calibration</Chip>
      </>
    ));
    const remove = screen.getByRole("button", { name: "Remove Brackets" });
    expect(remove.tagName).toBe("BUTTON");
    expect(remove).toHaveAttribute("type", "button");
    expect(remove.tabIndex).toBe(0);
    expect(screen.getByRole("button", { name: "Remove Calibration" })).toBeInTheDocument();
    // Never nested inside the chip's own toggle button.
    expect(remove.closest("button:not([aria-labelledby])")).toBeNull();
    expect(screen.getByRole("button", { name: "Brackets" })).toBeInTheDocument();
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

  it("renders an error message and describes the trigger with it", () => {
    render(() => <Select label="Fruit" options={["Apple", "Banana"]} error="Pick another fruit" />);

    const message = screen.getByText("Pick another fruit");
    expect(message.id).not.toBe("");
    const describedBy = screen.getByRole("button").getAttribute("aria-describedby") ?? "";
    expect(describedBy.split(" ")).toContain(message.id);
  });
});

describe("Select groups and an empty controlled value", () => {
  it("lists options under their group headings and selects one", async () => {
    const onChange = vi.fn();
    render(() => (
      <Select
        label="Target"
        placeholder="Choose a target"
        value={null}
        groups={[
          { label: "Printers", options: ["CC Left"] },
          { label: "Printer profiles", options: ["Centauri Carbon 0.4"] },
        ]}
        onChange={onChange}
      />
    ));
    expect(screen.getByRole("button")).toHaveTextContent("Choose a target");
    await fireEvent.pointerDown(screen.getByRole("button"), { pointerType: "mouse", button: 0 });
    expect(await screen.findByText("Printers")).toBeInTheDocument();
    expect(screen.getByText("Printer profiles")).toBeInTheDocument();
    await fireEvent.pointerUp(screen.getByRole("option", { name: "Centauri Carbon 0.4" }), { pointerType: "mouse", button: 0 });
    expect(onChange).toHaveBeenCalledWith("Centauri Carbon 0.4");
  });

  it("shows the placeholder again when the value goes back to null", () => {
    const [value, setValue] = createSignal<string | null>("Apple");
    render(() => <Select label="Fruit" placeholder="None" options={["Apple", "Banana"]} value={value()} />);
    expect(screen.getByRole("button")).toHaveTextContent("Apple");
    setValue(null);
    expect(screen.getByRole("button")).toHaveTextContent("None");
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

  it("with `multiple`, adds options to the value array, keeps the list open, and removes a chosen option", async () => {
    const [value, setValue] = createSignal<string[]>(["Prusa MK4"]);
    const onChange = vi.fn((next: string[]) => setValue(next));
    render(() => (
      <Combobox
        multiple
        label="Printers"
        options={["Elegoo Centauri Carbon", "Prusa MK4", "Voron 2.4"]}
        value={value()}
        onChange={onChange}
      />
    ));
    // The chosen value is shown as a removable token beside the input.
    expect(screen.getByRole("button", { name: "Remove Prusa MK4" })).toBeInTheDocument();

    const trigger = screen.getByRole("button", { name: /show suggestions/i });
    await fireEvent.pointerDown(trigger, { pointerType: "mouse", button: 0 });
    await fireEvent.pointerUp(await screen.findByRole("option", { name: "Voron 2.4" }), { pointerType: "mouse", button: 0 });
    expect(onChange).toHaveBeenLastCalledWith(["Prusa MK4", "Voron 2.4"]);
    // A multi-select stays open for the next pick, and marks what's chosen.
    const voron = screen.getByRole("option", { name: "Voron 2.4" });
    expect(voron).toHaveAttribute("aria-selected", "true");

    await fireEvent.pointerUp(screen.getByRole("option", { name: "Prusa MK4" }), { pointerType: "mouse", button: 0 });
    expect(onChange).toHaveBeenLastCalledWith(["Voron 2.4"]);

    await fireEvent.click(screen.getByRole("button", { name: "Remove Voron 2.4" }));
    expect(onChange).toHaveBeenLastCalledWith([]);
  });

  it("with `multiple`, puts each token's remove button in the tab order", () => {
    render(() => (
      <Combobox multiple label="Printers" options={["Prusa MK4", "Voron 2.4"]} value={["Prusa MK4", "Voron 2.4"]} />
    ));
    for (const name of ["Remove Prusa MK4", "Remove Voron 2.4"]) {
      const remove = screen.getByRole("button", { name });
      expect(remove.tagName).toBe("BUTTON");
      expect(remove.tabIndex).toBe(0);
    }
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

  it("renders no trigger button when trigger is omitted and is driven by `open` alone", async () => {
    render(() => (
      <Dialog title="Confirm" open>
        Body content
      </Dialog>
    ));

    expect(await screen.findByText("Body content")).toBeInTheDocument();
    // Only the Close button — no trigger.
    expect(screen.getAllByRole("button").map((b) => b.getAttribute("aria-label"))).toEqual(["Close"]);
  });

  it("returns focus to `returnFocus` when a trigger-less dialog closes", async () => {
    const [open, setOpen] = createSignal(false);
    render(() => (
      <>
        <button onClick={() => setOpen(true)}>Opener</button>
        <Dialog title="Confirm" open={open()} onOpenChange={setOpen} returnFocus={() => screen.getByText("Opener")}>
          Body content
        </Dialog>
      </>
    ));
    await fireEvent.click(screen.getByText("Opener"));
    await screen.findByText("Body content");
    await waitFor(() => expect(screen.getByRole("dialog").contains(document.activeElement)).toBe(true));
    await fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByText("Body content")).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByText("Opener")).toHaveFocus());
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

  it("shows a placeholder while empty", () => {
    render(() => <NumberField label="Walls" placeholder="Preset's value" />);
    expect(screen.getByLabelText("Walls")).toHaveAttribute("placeholder", "Preset's value");
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

describe("SeverityMarker", () => {
  it("renders an icon, visible label, and severity state", () => {
    const { container } = render(() => <SeverityMarker severity="fatal" label="Connection error" />);

    expect(screen.getByText("Connection error")).toBeVisible();
    expect(screen.getByRole("status", { name: "Connection error" })).toHaveAttribute(
      "data-severity",
      "fatal",
    );
    expect(container.querySelector("svg[aria-hidden='true']")).toBeInTheDocument();
  });
});

const rosterPrinters = Array.from({ length: 10 }, (_, index) => ({
  id: `printer-${index + 1}`,
  name: `Printer ${index + 1}`,
  stateLabel: index === 0 ? "Offline" : "Ready",
}));

describe("PrinterRoster", () => {
  it("shows each Printer's detail beside its state", async () => {
    render(() => (
      <PrinterRoster
        label="matching Printers"
        count={1}
        printers={[{ id: "a", name: "CC 1", detail: "Bench 1", stateLabel: "Ready" }]}
      />
    ));
    await fireEvent.click(screen.getByLabelText("1 matching Printers"));
    expect(await screen.findByText("· Bench 1")).toBeInTheDocument();
    expect(screen.getByText("Ready")).toBeInTheDocument();
  });

  it("keeps focus on the chip when it's focused, without opening", async () => {
    render(() => <PrinterRoster label="offline Printers" count={10} printers={rosterPrinters} />);

    const trigger = screen.getByLabelText("10 offline Printers");
    trigger.focus();
    await fireEvent.focus(trigger);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(document.activeElement).toBe(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("Printer 1")).not.toBeInTheDocument();
  });

  it("opens on press and returns focus to the chip after Escape", async () => {
    render(() => <PrinterRoster label="offline Printers" count={10} printers={rosterPrinters} />);

    const trigger = screen.getByLabelText("10 offline Printers");
    trigger.focus();
    // Enter and Space press a button through its click.
    await fireEvent.click(trigger);
    await waitFor(() => expect(screen.getByText("Printer 1")).toBeVisible());
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    await waitFor(() => expect(screen.getByRole("dialog", { hidden: true }).contains(document.activeElement)).toBe(true));

    await fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByText("Printer 1")).not.toBeInTheDocument();
      expect(trigger).toHaveFocus();
      expect(trigger).toHaveAttribute("aria-expanded", "false");
    });
  });

  it("continues the Tab order after the chip, either way, from the open roster", async () => {
    render(() => (
      <div>
        <button type="button">Before</button>
        <PrinterRoster label="Printers" count={10} printers={rosterPrinters} />
        <button type="button">After</button>
      </div>
    ));
    const trigger = screen.getByLabelText("10 Printers");

    await fireEvent.click(trigger);
    const viewAll = await screen.findByRole("button", { name: "View all", hidden: true });
    viewAll.focus();
    await fireEvent.keyDown(viewAll, { key: "Tab" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { hidden: true })).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "After", hidden: true })).toHaveFocus();
    });

    await fireEvent.click(trigger);
    const dialog = await screen.findByRole("dialog", { hidden: true });
    dialog.focus();
    await fireEvent.keyDown(dialog, { key: "Tab", shiftKey: true });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { hidden: true })).not.toBeInTheDocument();
      expect(trigger).toHaveFocus();
    });
  });

  it("moves on from a roster with nothing to press, when tabbed", async () => {
    render(() => (
      <div>
        <PrinterRoster label="Printers" count={2} printers={rosterPrinters.slice(0, 2)} />
        <button type="button" disabled>Disabled</button>
        <button type="button">After</button>
      </div>
    ));
    await fireEvent.click(screen.getByLabelText("2 Printers"));
    const dialog = await screen.findByRole("dialog", { hidden: true });
    await waitFor(() => expect(dialog).toHaveFocus());
    await fireEvent.keyDown(dialog, { key: "Tab" });
    await waitFor(() => expect(screen.getByRole("button", { name: "After", hidden: true })).toHaveFocus());
  });

  it("opens on hover without taking focus", async () => {
    render(() => (
      <div>
        <button type="button">Elsewhere</button>
        <PrinterRoster label="Printers" count={3} printers={rosterPrinters.slice(0, 3)} />
      </div>
    ));
    const elsewhere = screen.getByRole("button", { name: "Elsewhere", hidden: true });
    elsewhere.focus();
    const trigger = screen.getByLabelText("3 Printers");

    await fireEvent.pointerEnter(trigger, { pointerType: "mouse" });
    await waitFor(() => expect(screen.getByText("Printer 1")).toBeVisible());
    expect(elsewhere).toHaveFocus();

    await fireEvent.pointerLeave(trigger, { pointerType: "mouse" });
    await waitFor(() => expect(screen.queryByText("Printer 1")).not.toBeInTheDocument());
    expect(elsewhere).toHaveFocus();
  });

  it("stays open when the hovered chip is clicked", async () => {
    render(() => <PrinterRoster label="Printers" count={3} printers={rosterPrinters.slice(0, 3)} />);
    const trigger = screen.getByLabelText("3 Printers");
    await fireEvent.pointerEnter(trigger, { pointerType: "mouse" });
    await waitFor(() => expect(screen.getByText("Printer 1")).toBeVisible());
    await fireEvent.click(trigger);
    await fireEvent.pointerLeave(trigger, { pointerType: "mouse" });
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(screen.getByText("Printer 1")).toBeVisible();
  });

  it("opens on pointer hover and bounds rows with a View all action", async () => {
    const onViewAll = vi.fn();
    render(() => (
      <PrinterRoster
        label="Printers"
        count={10}
        printers={rosterPrinters}
        onViewAll={onViewAll}
      />
    ));

    const trigger = screen.getByLabelText("10 Printers");
    await fireEvent.pointerEnter(trigger, { pointerType: "mouse" });

    await waitFor(() => expect(screen.getByText("Printer 8")).toBeVisible());
    expect(screen.queryByText("Printer 9")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "View all" })).toBeVisible();

    await fireEvent.click(screen.getByRole("button", { name: "View all" }));
    expect(onViewAll).toHaveBeenCalledTimes(1);
  });

  it("omits View all when the total is eight or fewer", async () => {
    render(() => <PrinterRoster label="Printers" count={8} printers={rosterPrinters} />);

    await fireEvent.click(screen.getByLabelText("8 Printers"));
    await waitFor(() => expect(screen.getByText("Printer 8")).toBeVisible());
    expect(screen.queryByRole("button", { name: "View all" })).not.toBeInTheDocument();
  });
});

const stepperSteps: StepperStep[] = [
  { id: "identify", label: "Identify", state: "complete" },
  { id: "connect", label: "Connect", state: "complete" },
  { id: "confirm", label: "Confirm" },
];

describe("Stepper", () => {
  it("marks the current step aria-current=\"step\"", () => {
    render(() => <Stepper steps={stepperSteps} current="connect" />);

    expect(screen.getByText("Connect").closest("li")).toHaveAttribute("aria-current", "step");
    expect(screen.getByText("Identify").closest("li")).not.toHaveAttribute("aria-current");
    expect(screen.getByText("Confirm").closest("li")).not.toHaveAttribute("aria-current");
  });

  it("calls onSelect with the step id when a complete step is clicked", async () => {
    const onSelect = vi.fn();
    render(() => <Stepper steps={stepperSteps} current="confirm" onSelect={onSelect} />);

    await fireEvent.click(screen.getByRole("button", { name: "Identify" }));
    expect(onSelect).toHaveBeenCalledWith("identify");
  });

  it("does not render incomplete future steps as buttons", () => {
    render(() => <Stepper steps={stepperSteps} current="identify" onSelect={vi.fn()} />);

    expect(screen.queryByRole("button", { name: "Confirm" })).not.toBeInTheDocument();
    expect(screen.getByText("Confirm")).toBeInTheDocument();
  });
});

describe("Textarea", () => {
  it("renders its label and calls onChange when typed into", async () => {
    const onChange = vi.fn();
    render(() => <Textarea label="Notes" value="" onChange={onChange} />);

    expect(screen.getByText("Notes")).toBeInTheDocument();
    const field = screen.getByLabelText("Notes");
    await fireEvent.input(field, { target: { value: "hello" } });
    expect(onChange).toHaveBeenCalledWith("hello");
  });

  it("renders errorMessage with aria-invalid", () => {
    render(() => (
      <Textarea label="Notes" value="" onChange={vi.fn()} errorMessage="Notes are required" />
    ));

    expect(screen.getByText("Notes are required")).toBeInTheDocument();
    expect(screen.getByLabelText("Notes")).toHaveAttribute("aria-invalid", "true");
  });
});

describe("FileDropSurface", () => {
  it("renders a region with the given accessible label", () => {
    render(() => <FileDropSurface active={false} label="Import models" onChoose={vi.fn()} />);

    expect(screen.getByRole("region", { name: "Import models" })).toBeInTheDocument();
  });

  it("calls onChoose when the Choose files… button is clicked, via a native button", async () => {
    const onChoose = vi.fn();
    render(() => <FileDropSurface active={false} label="Import models" onChoose={onChoose} />);

    const button = screen.getByRole("button", { name: "Choose files…" }) as HTMLButtonElement;
    expect(button.tagName).toBe("BUTTON");
    expect(button).toHaveAttribute("type", "button");

    await fireEvent.click(button);
    expect(onChoose).toHaveBeenCalledTimes(1);
  });

  it("sets data-active when active", () => {
    render(() => <FileDropSurface active label="Import models" onChoose={vi.fn()} />);

    expect(screen.getByRole("region")).toHaveAttribute("data-active");
  });

  it("disables the button and shows the disabled reason as visible text", () => {
    render(() => (
      <FileDropSurface
        active={false}
        disabled
        disabledReason="Import already running"
        label="Import models"
        onChoose={vi.fn()}
      />
    ));

    const button = screen.getByRole("button", { name: "Choose files…" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect(screen.getByText("Import already running")).toBeVisible();
  });
});

describe("SegmentedControl", () => {
  it("renders a labelled group with each option's visible label", () => {
    render(() => (
      <SegmentedControl
        label="View"
        value="grid"
        options={[
          { value: "grid", label: "Grid" },
          { value: "list", label: "List" },
        ]}
        onChange={vi.fn()}
      />
    ));

    expect(screen.getByRole("radiogroup", { name: "View" })).toBeInTheDocument();
    expect(screen.getByText("Grid")).toBeInTheDocument();
    expect(screen.getByText("List")).toBeInTheDocument();
  });

  it("renders the option label as visible text even when an icon is supplied", () => {
    render(() => (
      <SegmentedControl
        label="View"
        value="grid"
        options={[
          { value: "grid", label: "Grid", icon: <svg aria-hidden="true" /> },
          { value: "list", label: "List" },
        ]}
        onChange={vi.fn()}
      />
    ));

    const label = screen.getByText("Grid");
    expect(label).toBeVisible();
  });

  it("marks the selected option with data-checked and updates it on selection", async () => {
    function Harness() {
      const [value, setValue] = createSignal<"grid" | "list">("grid");
      return (
        <SegmentedControl
          label="View"
          value={value()}
          options={[
            { value: "grid", label: "Grid" },
            { value: "list", label: "List" },
          ]}
          onChange={setValue}
        />
      );
    }
    render(() => <Harness />);

    expect(screen.getByText("Grid").closest("[role='group']")).toHaveAttribute("data-checked");
    expect(screen.getByText("List").closest("[role='group']")).not.toHaveAttribute(
      "data-checked",
    );

    await fireEvent.click(screen.getByText("List"));

    expect(screen.getByText("List").closest("[role='group']")).toHaveAttribute("data-checked");
    expect(screen.getByText("Grid").closest("[role='group']")).not.toHaveAttribute(
      "data-checked",
    );
  });

  it("renders each option as a native radio input sharing one name, so arrow-key and Space navigation between options is native browser behavior (not a hand-rolled keydown handler)", () => {
    render(() => (
      <SegmentedControl
        label="View"
        value="grid"
        options={[
          { value: "grid", label: "Grid" },
          { value: "list", label: "List" },
        ]}
        onChange={vi.fn()}
      />
    ));

    const inputs = document.querySelectorAll('input[type="radio"]');
    expect(inputs).toHaveLength(2);
    const names = new Set(Array.from(inputs).map((input) => (input as HTMLInputElement).name));
    expect(names.size).toBe(1);
  });

  it("focuses a segment's real input, immediately followed by its visible label — the DOM state the adjacent-sibling :focus-visible focus-ring CSS keys on", () => {
    render(() => (
      <SegmentedControl
        label="View"
        value="grid"
        options={[
          { value: "grid", label: "Grid" },
          { value: "list", label: "List" },
        ]}
        onChange={vi.fn()}
      />
    ));

    const listInput = document.querySelector('input[value="list"]') as HTMLInputElement;
    const listLabel = screen.getByText("List");

    // The focus-ring rule is `.input:focus-visible + .itemLabel`: it only
    // works if the real (Kobalte-rendered) input is focusable and is the
    // label's immediately preceding sibling.
    expect(listInput.nextElementSibling).toBe(listLabel);

    listInput.focus();
    expect(document.activeElement).toBe(listInput);
  });
});

describe("Progress", () => {
  it("reads its value as the given text", () => {
    render(() => <Progress label="Slicing" value={42} showValue valueLabel="42% of the plate" />);
    const bar = screen.getByRole("progressbar");
    expect(bar).toHaveAttribute("aria-valuenow", "42");
    expect(bar).toHaveAttribute("aria-valuetext", "42% of the plate");
    expect(screen.getByText("42% of the plate")).toBeInTheDocument();
  });

  it("has no value while indeterminate", () => {
    render(() => <Progress label="Slicing" indeterminate />);
    const bar = screen.getByRole("progressbar");
    expect(bar).not.toHaveAttribute("aria-valuenow");
    expect(bar).toHaveAttribute("data-indeterminate");
  });
});
