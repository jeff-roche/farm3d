/** Material Design 3 color roles, as hex strings (e.g. "#3c6e35"). */
export interface ColorRoles {
  primary: string;
  onPrimary: string;
  primaryContainer: string;
  onPrimaryContainer: string;
  secondary: string;
  onSecondary: string;
  secondaryContainer: string;
  onSecondaryContainer: string;
  tertiary: string;
  onTertiary: string;
  tertiaryContainer: string;
  onTertiaryContainer: string;
  error: string;
  onError: string;
  errorContainer: string;
  onErrorContainer: string;
  background: string;
  onBackground: string;
  surface: string;
  onSurface: string;
  surfaceVariant: string;
  onSurfaceVariant: string;
  outline: string;
  outlineVariant: string;
  shadow: string;
  scrim: string;
  inverseSurface: string;
  inverseOnSurface: string;
  inversePrimary: string;
  surfaceDim: string;
  surfaceBright: string;
  surfaceContainerLowest: string;
  surfaceContainerLow: string;
  surfaceContainer: string;
  surfaceContainerHigh: string;
  surfaceContainerHighest: string;
  surfaceTint: string;
}

export interface TypeStyle {
  fontFamily: string;
  fontWeight: number;
  /** CSS length, e.g. "2.25rem". */
  fontSize: string;
  /** CSS length, e.g. "2.75rem". */
  lineHeight: string;
  /** CSS length, e.g. "0em". */
  letterSpacing: string;
}

/** The M3 type scale: 5 roles (display/headline/title/body/label) x 3 sizes. */
export interface TypographyScale {
  displayLarge: TypeStyle;
  displayMedium: TypeStyle;
  displaySmall: TypeStyle;
  headlineLarge: TypeStyle;
  headlineMedium: TypeStyle;
  headlineSmall: TypeStyle;
  titleLarge: TypeStyle;
  titleMedium: TypeStyle;
  titleSmall: TypeStyle;
  bodyLarge: TypeStyle;
  bodyMedium: TypeStyle;
  bodySmall: TypeStyle;
  labelLarge: TypeStyle;
  labelMedium: TypeStyle;
  labelSmall: TypeStyle;
}

/** The M3 corner-radius scale, as CSS length strings. */
export interface ShapeScale {
  none: string;
  extraSmall: string;
  small: string;
  medium: string;
  large: string;
  extraLarge: string;
  full: string;
}

export interface Theme {
  /** Unique registry key, e.g. "material-light". */
  name: string;
  /** Whether this theme is meant for light or dark surfaces; used to pick a theme for 'system' mode. */
  scheme: "light" | "dark";
  color: ColorRoles;
  typography: TypographyScale;
  shape: ShapeScale;
}
