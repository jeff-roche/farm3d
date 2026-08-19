import type { TypographyScale, TypeStyle } from "./types";

const plain = "Roboto, Inter, Avenir, Helvetica, Arial, sans-serif";

function style(
  fontWeight: number,
  fontSize: string,
  lineHeight: string,
  letterSpacing: string,
): TypeStyle {
  return { fontFamily: plain, fontWeight, fontSize, lineHeight, letterSpacing };
}

/** The M3 baseline type scale (https://m3.material.io/styles/typography/type-scale-tokens). */
export const m3Typography: TypographyScale = {
  displayLarge: style(400, "3.5625rem", "4rem", "-0.015625em"),
  displayMedium: style(400, "2.8125rem", "3.25rem", "0em"),
  displaySmall: style(400, "2.25rem", "2.75rem", "0em"),

  headlineLarge: style(400, "2rem", "2.5rem", "0em"),
  headlineMedium: style(400, "1.75rem", "2.25rem", "0em"),
  headlineSmall: style(400, "1.5rem", "2rem", "0em"),

  titleLarge: style(400, "1.375rem", "1.75rem", "0em"),
  titleMedium: style(500, "1rem", "1.5rem", "0.009375em"),
  titleSmall: style(500, "0.875rem", "1.25rem", "0.00625em"),

  bodyLarge: style(400, "1rem", "1.5rem", "0.03125em"),
  bodyMedium: style(400, "0.875rem", "1.25rem", "0.015625em"),
  bodySmall: style(400, "0.75rem", "1rem", "0.025em"),

  labelLarge: style(500, "0.875rem", "1.25rem", "0.00625em"),
  labelMedium: style(500, "0.75rem", "1rem", "0.03125em"),
  labelSmall: style(500, "0.6875rem", "1rem", "0.03125em"),
};
