import type { Theme } from "../tokens/types";
import { editorTypography } from "../tokens/typography";
import { editorShape } from "../tokens/shape";

/**
 * The default dark theme — an editor/tool aesthetic (Blender/Godot/Unity-inspired)
 * rather than a consumer-app one: flat, dense, neutral grays with a farm-green accent.
 */
export const darkTheme: Theme = {
  name: "editor-dark",
  scheme: "dark",
  color: {
    bg: "#17181a",
    surface: "#1e2023",
    surfaceRaised: "#262a2e",
    surfaceHover: "#2c3034",
    surfaceSelected: "#2f3d24",

    border: "#34383c",
    borderStrong: "#45494e",

    text: "#e8e6e0",
    textMuted: "#9a9d98",
    textDisabled: "#5c5f5b",

    accent: "#8fc46b",
    onAccent: "#14210b",
    accentMuted: "#24361b",

    danger: "#e5484d",
    onDanger: "#ffffff",
    warning: "#e2a336",
    onWarning: "#2a1c02",
    success: "#3fb37f",
    onSuccess: "#062015",

    focusRing: "#8fc46b",
  },
  typography: editorTypography,
  shape: editorShape,
};
