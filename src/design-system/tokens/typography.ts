import type { TypographyScale, TypeStyle } from "./types";

const ui = "Inter, Avenir, Helvetica, Arial, sans-serif";
const mono = "'JetBrains Mono', 'SF Mono', Menlo, Consolas, monospace";

function style(
  fontFamily: string,
  fontWeight: number,
  fontSize: string,
  lineHeight: string,
  letterSpacing: string,
): TypeStyle {
  return { fontFamily, fontWeight, fontSize, lineHeight, letterSpacing };
}

/** Dense, editor-style type scale — smaller sizes and tighter line-height than a consumer-app scale. */
export const editorTypography: TypographyScale = {
  heading: style(ui, 600, "0.8125rem", "1.25rem", "0.02em"),
  body: style(ui, 400, "0.8125rem", "1.25rem", "0em"),
  bodySmall: style(ui, 400, "0.75rem", "1.125rem", "0em"),
  label: style(ui, 500, "0.6875rem", "1rem", "0.03em"),
  mono: style(mono, 400, "0.75rem", "1.125rem", "0em"),
};
