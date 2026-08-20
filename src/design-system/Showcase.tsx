import { createSignal } from "solid-js";
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
} from ".";
import styles from "./Showcase.module.css";

/** Renders every design-system component and its states, for visual QA. Dev-only. */
export function Showcase() {
  const theme = useTheme();
  const [checked, setChecked] = createSignal(true);
  const [switched, setSwitched] = createSignal(false);
  const [sliderValue, setSliderValue] = createSignal(40);
  const [radioValue, setRadioValue] = createSignal("b");
  const [chipSelected, setChipSelected] = createSignal(true);
  const [numberValue, setNumberValue] = createSignal(120);

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
        <Select label="Fruit" options={["Apple", "Banana", "Cherry"]} defaultValue="Banana" />
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
        <NumberField
          label="Bed height"
          suffix="mm"
          minValue={0}
          maxValue={500}
          step={1}
          value={numberValue()}
          onChange={setNumberValue}
        />
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
        </div>
      </Panel>
    </div>
  );
}
