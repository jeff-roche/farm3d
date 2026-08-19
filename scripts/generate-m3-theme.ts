/**
 * Generates src/design-system/themes/{light,dark}.ts from a seed color,
 * using Google's own M3 HCT color algorithm (@material/material-color-utilities).
 *
 * This is a one-off authoring tool, not part of the app build. Re-run it
 * (`npm run generate:theme`) only when the seed color changes; the output
 * is committed as plain static Theme objects.
 */
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  Hct,
  SchemeTonalSpot,
  argbFromHex,
  hexFromArgb,
} from "@material/material-color-utilities";
import type { ColorRoles } from "../src/design-system/tokens/types";

/** Farm-themed seed color (earthy green) in place of M3's default purple. */
const SEED_COLOR = "#4C7A2A";

function colorRolesFromScheme(scheme: SchemeTonalSpot): ColorRoles {
  const hex = (argb: number) => hexFromArgb(argb);
  return {
    primary: hex(scheme.primary),
    onPrimary: hex(scheme.onPrimary),
    primaryContainer: hex(scheme.primaryContainer),
    onPrimaryContainer: hex(scheme.onPrimaryContainer),
    secondary: hex(scheme.secondary),
    onSecondary: hex(scheme.onSecondary),
    secondaryContainer: hex(scheme.secondaryContainer),
    onSecondaryContainer: hex(scheme.onSecondaryContainer),
    tertiary: hex(scheme.tertiary),
    onTertiary: hex(scheme.onTertiary),
    tertiaryContainer: hex(scheme.tertiaryContainer),
    onTertiaryContainer: hex(scheme.onTertiaryContainer),
    error: hex(scheme.error),
    onError: hex(scheme.onError),
    errorContainer: hex(scheme.errorContainer),
    onErrorContainer: hex(scheme.onErrorContainer),
    background: hex(scheme.background),
    onBackground: hex(scheme.onBackground),
    surface: hex(scheme.surface),
    onSurface: hex(scheme.onSurface),
    surfaceVariant: hex(scheme.surfaceVariant),
    onSurfaceVariant: hex(scheme.onSurfaceVariant),
    outline: hex(scheme.outline),
    outlineVariant: hex(scheme.outlineVariant),
    shadow: hex(scheme.shadow),
    scrim: hex(scheme.scrim),
    inverseSurface: hex(scheme.inverseSurface),
    inverseOnSurface: hex(scheme.inverseOnSurface),
    inversePrimary: hex(scheme.inversePrimary),
    surfaceDim: hex(scheme.surfaceDim),
    surfaceBright: hex(scheme.surfaceBright),
    surfaceContainerLowest: hex(scheme.surfaceContainerLowest),
    surfaceContainerLow: hex(scheme.surfaceContainerLow),
    surfaceContainer: hex(scheme.surfaceContainer),
    surfaceContainerHigh: hex(scheme.surfaceContainerHigh),
    surfaceContainerHighest: hex(scheme.surfaceContainerHighest),
    surfaceTint: hex(scheme.surfaceTint),
  };
}

function renderThemeFile(
  themeName: string,
  schemeMode: "light" | "dark",
  color: ColorRoles,
): string {
  const colorEntries = Object.entries(color)
    .map(([key, value]) => `    ${key}: "${value}",`)
    .join("\n");

  return `// GENERATED FILE — do not edit by hand.
// Regenerate with \`npm run generate:theme\` (see scripts/generate-m3-theme.ts).
import type { Theme } from "../tokens/types";
import { m3Typography } from "../tokens/typography";
import { m3Shape } from "../tokens/shape";

export const ${themeName}: Theme = {
  name: "material-${schemeMode}",
  scheme: "${schemeMode}",
  color: {
${colorEntries}
  },
  typography: m3Typography,
  shape: m3Shape,
};
`;
}

function main() {
  const sourceHct = Hct.fromInt(argbFromHex(SEED_COLOR));

  const lightScheme = new SchemeTonalSpot(sourceHct, false, 0);
  const darkScheme = new SchemeTonalSpot(sourceHct, true, 0);

  const themesDir = fileURLToPath(
    new URL("../src/design-system/themes/", import.meta.url),
  );

  writeFileSync(
    `${themesDir}light.ts`,
    renderThemeFile("lightTheme", "light", colorRolesFromScheme(lightScheme)),
  );
  writeFileSync(
    `${themesDir}dark.ts`,
    renderThemeFile("darkTheme", "dark", colorRolesFromScheme(darkScheme)),
  );

  console.log(`Generated light.ts and dark.ts from seed ${SEED_COLOR}`);
}

main();
