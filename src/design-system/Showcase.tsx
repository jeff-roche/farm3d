import { createSignal } from "solid-js";
import { IconLayoutGrid, IconLayoutList } from "@tabler/icons-solidjs";
import {
  useTheme,
  Panel,
  Button,
  IconButton,
  Chip,
  TextField,
  Select,
  Combobox,
  Checkbox,
  RadioGroup,
  Switch,
  Slider,
  Tabs,
  Dialog,
  Popover,
  Tooltip,
  DropdownMenu,
  Progress,
  Logo,
  NumberField,
  Field,
  SeverityMarker,
  PrinterRoster,
  Stepper,
  Textarea,
  DataTable,
  Timeline,
  ColorSwatch,
  FileDropSurface,
  SegmentedControl,
  type DataTableColumn,
  type DataTableSort,
} from ".";
import styles from "./Showcase.module.css";

/** Renders every design-system component and its states, for visual QA. Dev-only. */
export function Showcase() {
  const theme = useTheme();
  const [checked, setChecked] = createSignal(true);
  const [switched, setSwitched] = createSignal(false);
  const [sliderValue, setSliderValue] = createSignal(40);
  const [radioValue, setRadioValue] = createSignal("b");
  const [projects, setProjects] = createSignal(["Brackets"]);
  const [chipSelected, setChipSelected] = createSignal(true);
  const [numberValue, setNumberValue] = createSignal(120);
  const [groupedTarget, setGroupedTarget] = createSignal<string | null>(null);
  const [dropActive, setDropActive] = createSignal(false);
  const [viewMode, setViewMode] = createSignal<"grid" | "list">("grid");
  const [stepperCurrent, setStepperCurrent] = createSignal("connect");
  const [textareaValue, setTextareaValue] = createSignal("");

  interface ShowcaseSpool {
    id: string;
    number: number;
    material: string;
    colorHex: string | null;
    colorName: string;
    remainingG: number;
  }

  const spoolRows: ShowcaseSpool[] = [
    { id: "1", number: 7, material: "PETG", colorHex: "#1c1c1c", colorName: "Black", remainingG: 842 },
    { id: "2", number: 12, material: "PLA", colorHex: "#2f7a3c", colorName: "Farm Green", remainingG: 210 },
    { id: "3", number: 3, material: "ABS", colorHex: null, colorName: "Unknown", remainingG: 1180 },
  ];

  const spoolColumns: DataTableColumn<ShowcaseSpool>[] = [
    { id: "number", header: "#", cell: (row) => `#${row.number}`, sortValue: (row) => row.number, width: "3rem" },
    { id: "material", header: "Material", cell: (row) => row.material, sortValue: (row) => row.material },
    {
      id: "color",
      header: "Color",
      cell: (row) => (
        <span style={{ display: "inline-flex", "align-items": "center", gap: "0.375rem" }}>
          <ColorSwatch hex={row.colorHex} name={row.colorName} size="sm" />
          {row.colorName}
        </span>
      ),
    },
    {
      id: "remaining",
      header: "Remaining",
      cell: (row) => `${row.remainingG} g`,
      sortValue: (row) => row.remainingG,
      align: "end",
    },
  ];

  const [spoolSelectedId, setSpoolSelectedId] = createSignal<string | null>("1");
  const [spoolSort, setSpoolSort] = createSignal<DataTableSort | undefined>(undefined);

  return (
    <div class={styles.page}>
      <Panel title="Theme">
        <Select
          label="Mode"
          options={["system", ...theme.availableThemes().map((t) => t.name)]}
          value={theme.mode()}
          onChange={theme.setThemeMode}
        />
      </Panel>

      <Panel title="Logo">
        <div class={styles.row}>
          <Logo size={16} />
          <Logo size={24} />
          <Logo size={48} title="farm3d" />
        </div>
      </Panel>

      <Panel title="Button">
        <div class={styles.row}>
          <Button variant="primary">Primary</Button>
          <Button variant="secondary">Secondary</Button>
          <Button variant="ghost">Ghost</Button>
          <Button variant="danger">Danger</Button>
          <Button variant="primary" disabled>
            Disabled
          </Button>
          <Button variant="secondary" size="sm">
            Small
          </Button>
        </div>
      </Panel>

      <Panel title="IconButton">
        <div class={styles.row}>
          <IconButton aria-label="Add">+</IconButton>
          <IconButton aria-label="Active" active>
            +
          </IconButton>
          <IconButton aria-label="Disabled" disabled>
            +
          </IconButton>
        </div>
      </Panel>

      <Panel title="Chip">
        <div class={styles.row}>
          <Chip selected={chipSelected()} onSelectedChange={setChipSelected}>
            Toggleable
          </Chip>
          <Chip onRemove={() => {}}>Removable</Chip>
          <Chip disabled>Disabled</Chip>
        </div>
      </Panel>

      <Panel title="TextField">
        <div class={styles.column}>
          <TextField label="Name" placeholder="Enter a name..." />
          <TextField label="With error" error="This field is required" />
          <TextField label="Disabled" disabled placeholder="Can't touch this" />
        </div>
      </Panel>

      <Panel title="Select">
        <div class={styles.column}>
          <Select label="Fruit" options={["Apple", "Banana", "Cherry"]} defaultValue="Banana" />
          <Select label="With error" options={["Apple", "Banana", "Cherry"]} error="That option no longer exists." />
          <Select
            label="Grouped"
            placeholder="Choose a target"
            value={groupedTarget()}
            onChange={setGroupedTarget}
            groups={[
              { label: "Printers", options: ["CC Left", "CC Right"] },
              { label: "Printer profiles", options: ["Elegoo Centauri Carbon 0.4 nozzle"] },
            ]}
          />
        </div>
      </Panel>

      <Panel title="Combobox">
        <div class={styles.column}>
          <Combobox label="Fruit" options={["Apple", "Banana", "Cherry"]} />
          <Combobox
            label="Printer model"
            groups={[
              { label: "Elegoo", options: ["Centauri Carbon", "Neptune 4"] },
              { label: "Prusa", options: ["MK4", "CORE One"] },
            ]}
          />
          <Combobox
            multiple
            label="Projects (multiple)"
            placeholder={projects().length === 0 ? "Unfiled" : undefined}
            options={["Brackets", "Calibration", "Enclosure parts"]}
            value={projects()}
            onChange={setProjects}
          />
        </div>
      </Panel>

      <Panel title="Checkbox">
        <div class={styles.row}>
          <Checkbox checked={checked()} onChange={setChecked}>
            Checked
          </Checkbox>
          <Checkbox indeterminate>Indeterminate</Checkbox>
          <Checkbox disabled>Disabled</Checkbox>
        </div>
      </Panel>

      <Panel title="RadioGroup">
        <RadioGroup
          label="Choose one"
          options={[
            { value: "a", label: "Option A" },
            { value: "b", label: "Option B" },
            { value: "c", label: "Option C" },
          ]}
          value={radioValue()}
          onChange={setRadioValue}
        />
      </Panel>

      <Panel title="Switch">
        <div class={styles.row}>
          <Switch checked={switched()} onChange={setSwitched}>
            Enabled
          </Switch>
          <Switch disabled>Disabled</Switch>
        </div>
      </Panel>

      <Panel title="Slider">
        <div class={styles.column}>
          <Slider
            label="Value"
            showValue
            value={sliderValue()}
            onChange={setSliderValue}
          />
        </div>
      </Panel>

      <Panel title="NumberField">
        <div class={styles.column}>
          <NumberField
            label="Bed height"
            suffix="mm"
            minValue={0}
            maxValue={500}
            step={1}
            value={numberValue()}
            onChange={setNumberValue}
          />
          <NumberField label="Walls" minValue={1} maxValue={20} step={1} placeholder="Preset's value" />
        </div>
      </Panel>

      <Panel title="Field">
        <div class={styles.row}>
          <Field label="Printable height">256 mm</Field>
          <Field label="Printable height" overridden onRevert={() => {}} hint="inherited: 256">
            240 mm
          </Field>
        </div>
      </Panel>

      <Panel title="Tabs">
        <Tabs
          items={[
            { value: "scene", label: "Scene", content: "Scene tree contents." },
            { value: "props", label: "Properties", content: "Selected object properties." },
            { value: "assets", label: "Assets", content: "Project asset browser." },
          ]}
          defaultValue="scene"
        />
      </Panel>

      <Panel title="Dialog">
        <Dialog title="Confirm action" description="This can't be undone." trigger="Open dialog">
          <div class={styles.row}>
            <Button variant="secondary">Cancel</Button>
            <Button variant="danger">Confirm</Button>
          </div>
        </Dialog>
      </Panel>

      <Panel title="Popover">
        <Popover trigger="Open popover">Non-modal popover content.</Popover>
      </Panel>

      <Panel title="Tooltip">
        <Tooltip trigger="Hover me">Helpful context here.</Tooltip>
      </Panel>

      <Panel title="DropdownMenu">
        <DropdownMenu
          trigger="Open menu"
          items={[
            { label: "Rename", onSelect: () => {} },
            { label: "Duplicate", onSelect: () => {} },
            { type: "separator" },
            { label: "Delete", onSelect: () => {} },
          ]}
        />
      </Panel>

      <Panel title="Progress">
        <div class={styles.column}>
          <Progress label="Loading assets" showValue value={65} />
          <Progress label="Indeterminate" indeterminate />
          <Progress label="Generating G-code" showValue value={42} valueLabel="42% of the plate" />
        </div>
      </Panel>

      <Panel title="SeverityMarker">
        <div class={styles.row}>
          <SeverityMarker severity="fatal" label="Connection error" />
          <SeverityMarker severity="warning" label="Cache unavailable" />
          <SeverityMarker severity="info" label="Telemetry unavailable" />
          <SeverityMarker severity="resolved" label="Ready" />
        </div>
      </Panel>

      <Panel title="PrinterRoster">
        <div class={styles.rosterExamples}>
          <PrinterRoster label="Printers" count={0} printers={[]} />
          <PrinterRoster
            label="offline Printers"
            count={3}
            printers={[
              { id: "1", name: "Atlas", detail: "Bench 1", stateLabel: "Offline" },
              { id: "2", name: "Forge", stateLabel: "Offline" },
              { id: "3", name: "Nova", stateLabel: "Offline" },
            ]}
          />
          <PrinterRoster
            label="Printers"
            count={10}
            printers={Array.from({ length: 10 }, (_, index) => ({
              id: String(index + 1),
              name: `Printer ${index + 1}`,
              stateLabel: index % 2 === 0 ? "Ready" : "Printing",
            }))}
            onViewAll={() => {}}
          />
        </div>
      </Panel>

      <Panel title="Stepper">
        <div class={styles.column}>
          <Stepper
            aria-label="Batch setup steps"
            steps={[
              { id: "identify", label: "Identify", state: "complete" },
              { id: "connect", label: "Connect", state: "complete" },
              { id: "confirm", label: "Confirm" },
            ]}
            current={stepperCurrent()}
            onSelect={setStepperCurrent}
          />
          <Stepper
            aria-label="Steps with an error"
            steps={[
              { id: "identify", label: "Identify", state: "complete" },
              { id: "connect", label: "Connect", state: "error" },
              { id: "confirm", label: "Confirm" },
            ]}
            current="connect"
          />
        </div>
      </Panel>

      <Panel title="Textarea">
        <div class={styles.column}>
          <Textarea
            label="Notes"
            placeholder="Anything worth remembering about this batch..."
            value={textareaValue()}
            onChange={setTextareaValue}
          />
          <Textarea
            label="Notes with error"
            value=""
            onChange={() => {}}
            errorMessage="Notes are required"
          />
        </div>
      </Panel>

      <Panel title="DataTable">
        <div class={styles.column}>
          <DataTable
            label="Spools"
            rows={spoolRows}
            rowId={(row) => row.id}
            columns={spoolColumns}
            selectedId={spoolSelectedId()}
            onSelect={setSpoolSelectedId}
            onActivate={(id) => console.log("activate", id)}
            sort={spoolSort()}
            onSortChange={setSpoolSort}
          />
          <DataTable
            label="Empty spools"
            rows={[]}
            rowId={(row: ShowcaseSpool) => row.id}
            columns={spoolColumns}
            empty={<span>No Spools yet — Add Spool</span>}
          />
        </div>
      </Panel>

      <Panel title="Timeline">
        <Timeline
          label="Spool history"
          items={[
            { id: "1", at: "2026-09-22T14:05:00Z", title: "Loaded into Atlas, slot 1" },
            {
              id: "2",
              at: "2026-09-21T09:30:00Z",
              title: "Recorded amount",
              detail: <span>842 g remaining (est.)</span>,
              marker: "muted",
            },
            { id: "3", at: "2026-09-18T11:00:00Z", title: "Moved to storage: Shelf B" },
          ]}
        />
      </Panel>

      <Panel title="ColorSwatch">
        <div class={styles.column}>
          <div class={styles.row}>
            <ColorSwatch hex="#1c1c1c" name="Black" />
            Black
          </div>
          <div class={styles.row}>
            <ColorSwatch hex="#2f7a3c" name="Farm Green" />
            Farm Green
          </div>
          <div class={styles.row}>
            <ColorSwatch hex={null} name="Unknown" />
            Unknown
          </div>
          <div class={styles.row}>
            <ColorSwatch hex="#2f7a3c" name="Farm Green" size="sm" />
            Farm Green (sm)
          </div>
        </div>
      </Panel>

      <Panel title="FileDropSurface">
        <div class={styles.column}>
          <FileDropSurface
            label="Import models"
            hint="Drag files here, or choose files to import."
            active={dropActive()}
            onChoose={() => setDropActive((value) => !value)}
          />
          <FileDropSurface
            label="Import models (disabled)"
            disabled
            disabledReason="An import is already running."
            active={false}
            onChoose={() => {}}
          />
        </div>
      </Panel>

      <Panel title="SegmentedControl">
        <SegmentedControl
          label="View"
          value={viewMode()}
          onChange={setViewMode}
          options={[
            { value: "grid", label: "Grid", icon: <IconLayoutGrid size={14} /> },
            { value: "list", label: "List", icon: <IconLayoutList size={14} /> },
          ]}
        />
      </Panel>
    </div>
  );
}
