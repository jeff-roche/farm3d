/** The Settings workspace's categories, in umbrella order (D16). A slug is
 *  the `settingsCategory` selection id, so a deep link reads
 *  `#nav=v1/settings/settingsCategory/<slug>`. */
export const SETTINGS_CATEGORIES = [
  { slug: "general", label: "General" },
  { slug: "appearance", label: "Appearance" },
  { slug: "slicing", label: "Slicing" },
  { slug: "notifications", label: "Notifications and retention" },
  { slug: "storage", label: "Storage and backup" },
  { slug: "connections", label: "Connections" },
  { slug: "diagnostics", label: "Diagnostics" },
  { slug: "about", label: "About" },
] as const;

export type SettingsCategorySlug = (typeof SETTINGS_CATEGORIES)[number]["slug"];

export const DEFAULT_SETTINGS_CATEGORY: SettingsCategorySlug = "general";

export const settingsCategorySlugs = (): string[] => SETTINGS_CATEGORIES.map((category) => category.slug);

/** An unknown or missing slug falls back to General. */
export function resolveSettingsCategory(slug: string | undefined): SettingsCategorySlug {
  return SETTINGS_CATEGORIES.find((category) => category.slug === slug)?.slug ?? DEFAULT_SETTINGS_CATEGORY;
}
