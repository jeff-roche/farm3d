import { lazy, Suspense } from "solid-js";
import { Tabs, type TabItem } from "../../design-system";
import type { MonitorStore } from "../../monitor/monitor-store";
import { resolveSettingsCategory, SETTINGS_CATEGORIES, type SettingsCategorySlug } from "./categories";
import styles from "./SettingsWorkspace.module.css";

// Each category is its own chunk: the workspace itself is lazy, and its
// heavier forms load only when their category is selected.
const GeneralSettings = lazy(() => import("./GeneralSettings").then((m) => ({ default: m.GeneralSettings })));
const AppearanceSettings = lazy(() => import("./AppearanceSettings").then((m) => ({ default: m.AppearanceSettings })));
const SlicerSettingsForm = lazy(() => import("./SlicerSettingsForm").then((m) => ({ default: m.SlicerSettingsForm })));
const NotificationSettingsForm = lazy(() => import("./NotificationSettingsForm").then((m) => ({ default: m.NotificationSettingsForm })));
const ConnectionsSettings = lazy(() => import("./ConnectionsSettings").then((m) => ({ default: m.ConnectionsSettings })));
const StorageSettings = lazy(() => import("./StorageSettings").then((m) => ({ default: m.StorageSettings })));
const DiagnosticsSettings = lazy(() => import("./DiagnosticsSettings").then((m) => ({ default: m.DiagnosticsSettings })));
const AboutSettings = lazy(() => import("./AboutSettings").then((m) => ({ default: m.AboutSettings })));

export interface SettingsWorkspaceProps {
  /** The `settingsCategory` selection's id. An unknown or missing slug
   *  shows General. */
  category?: string;
  onCategoryChange: (slug: SettingsCategorySlug) => void;
  /** The Monitor's store, for General's section and density. */
  monitor?: MonitorStore;
  /** Forwarded to Connections after a Printers import. */
  onPrintersImported?: () => void;
  /** Opens the Queue, for a restore that active work blocks. */
  onOpenQueue?: () => void;
}

/** The Settings destination (D16): the categories as vertical tabs, the
 *  selected one following the navigation selection. Leaving a category
 *  unmounts it, which is what drops a pending theme preview. */
export function SettingsWorkspace(props: SettingsWorkspaceProps) {
  const content: Record<SettingsCategorySlug, () => TabItem["content"]> = {
    general: () => <GeneralSettings monitor={props.monitor} />,
    appearance: () => <AppearanceSettings />,
    slicing: () => <SlicerSettingsForm />,
    notifications: () => <NotificationSettingsForm />,
    connections: () => <ConnectionsSettings onPrintersImported={props.onPrintersImported} />,
    storage: () => <StorageSettings onOpenQueue={props.onOpenQueue} />,
    diagnostics: () => <DiagnosticsSettings />,
    about: () => <AboutSettings />,
  };
  const items = (): TabItem[] =>
    SETTINGS_CATEGORIES.map((category) => ({
      value: category.slug,
      label: category.label,
      // A getter, so a category is built only while it is selected and is
      // torn down (dropping any pending theme preview) when it isn't.
      get content() {
        return (
          <Suspense fallback={<p class={styles.loading} role="status">Loading…</p>}>{content[category.slug]()}</Suspense>
        );
      },
    }));

  return (
    <div class={styles.workspace}>
      <Tabs
        orientation="vertical"
        class={styles.tabs}
        items={items()}
        value={resolveSettingsCategory(props.category)}
        onChange={(slug) => props.onCategoryChange(resolveSettingsCategory(slug))}
      />
    </div>
  );
}
