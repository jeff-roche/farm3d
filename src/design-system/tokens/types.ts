/** A small, editor-style color palette (~19 roles), as hex strings (e.g. "#4c7a2a"). */
export interface ColorRoles {
  /** Outermost app background, behind panels. */
  bg: string;
  /** Base panel/card background. */
  surface: string;
  /** Dialog/popover/dropdown/menu background. */
  surfaceRaised: string;
  /** Hover background for interactive rows/items. */
  surfaceHover: string;
  /** Selected/pressed background for interactive rows/items. */
  surfaceSelected: string;

  /** Subtle dividers/panel outlines. */
  border: string;
  /** More visible border, e.g. default input border. */
  borderStrong: string;

  text: string;
  textMuted: string;
  textDisabled: string;

  accent: string;
  /** Text/icon color atop an accent-filled surface. */
  onAccent: string;
  /** Subtle accent background, e.g. hover on an accent button. */
  accentMuted: string;

  danger: string;
  onDanger: string;
  warning: string;
  onWarning: string;
  success: string;
  onSuccess: string;

  /** Keyboard-focus outline color. */
  focusRing: string;
}

export interface TypeStyle {
  fontFamily: string;
  fontWeight: number;
  /** CSS length, e.g. "0.875rem". */
  fontSize: string;
  /** CSS length, e.g. "1.25rem". */
  lineHeight: string;
  /** CSS length, e.g. "0em". */
  letterSpacing: string;
}

/** A lean type scale: panel headers, body text, help text, form labels, and numeric readouts. */
export interface TypographyScale {
  heading: TypeStyle;
  body: TypeStyle;
  bodySmall: TypeStyle;
  label: TypeStyle;
  /** Monospace, for coordinate/transform/property value readouts. */
  mono: TypeStyle;
}

/** A lean corner-radius scale, as CSS length strings. */
export interface ShapeScale {
  none: string;
  sm: string;
  md: string;
  full: string;
}

export interface Theme {
  /** Unique registry key, e.g. "editor-light". */
  name: string;
  /** Whether this theme is meant for light or dark surfaces; used to pick a theme for 'system' mode. */
  scheme: "light" | "dark";
  color: ColorRoles;
  typography: TypographyScale;
  shape: ShapeScale;
}
