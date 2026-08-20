import type { Theme } from "../tokens/types";
import { farm3dTypography } from "../tokens/typography";
import { farm3dShape } from "../tokens/shape";

/** The light counterpart to darkTheme — same editor aesthetic and farm-green accent. */
export const lightTheme: Theme = {
  name: "farm3d-light",
  scheme: "light",
  color: {
    bg: "#eef0ea",
    surface: "#f7f8f4",
    surfaceRaised: "#ffffff",
    surfaceHover: "#eceee7",
    surfaceSelected: "#ddebd2",

    border: "#d7dad0",
    borderStrong: "#b7bcae",

    text: "#1b1e18",
    textMuted: "#5b5f56",
    textDisabled: "#9a9e93",

    accent: "#47672f",
    onAccent: "#ffffff",
    accentMuted: "#e2ecd7",

    danger: "#c22a2a",
    onDanger: "#ffffff",
    warning: "#a3690a",
    onWarning: "#ffffff",
    success: "#2f7d52",
    onSuccess: "#ffffff",

    focusRing: "#47672f",
  },
  typography: farm3dTypography,
  shape: farm3dShape,
};
