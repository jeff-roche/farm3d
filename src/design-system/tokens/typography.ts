import type { TypographyScale, TypeStyle } from "./types";

const heading = "'Manrope', system-ui, sans-serif";
const body = "'Fira Sans', system-ui, sans-serif";
const mono = "'Fira Code', 'SF Mono', Menlo, Consolas, monospace";

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
export const farm3dTypography: TypographyScale = {
  heading: style(heading, 600, "0.8125rem", "1.25rem", "0.02em"),
  body: style(body, 400, "0.8125rem", "1.25rem", "0em"),
  bodySmall: style(body, 400, "0.75rem", "1.125rem", "0em"),
  label: style(body, 500, "0.6875rem", "1rem", "0.03em"),
  mono: style(mono, 400, "0.75rem", "1.125rem", "0em"),
};
