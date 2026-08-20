import { createUniqueId, type JSX } from "solid-js";

export interface LogoProps {
  /** Rendered width/height in px. Defaults to 24. */
  size?: number;
  class?: string;
  /** Accessible name. Omit for decorative use (e.g. next to visible text). */
  title?: string;
}

/** farm3d's icon mark: a layer-stack cube with a sprout, fixed brand colors (not theme tokens). */
export function Logo(props: LogoProps): JSX.Element {
  const size = () => props.size ?? 24;
  const clipId = `f3d-logo-clip-${createUniqueId()}`;

  return (
    <svg
      width={size()}
      height={size()}
      viewBox="0 0 100 100"
      class={props.class}
      role={props.title ? "img" : undefined}
      aria-label={props.title}
      aria-hidden={props.title ? undefined : "true"}
    >
      <defs>
        <clipPath id={clipId}>
          <path d="M44.3,11.15 Q50,8 55.7,11.15 L82.3,25.85 Q88,29 88,35.3 L88,64.7 Q88,71 82.3,74.15 L55.7,88.85 Q50,92 44.3,88.85 L17.7,74.15 Q12,71 12,64.7 L12,35.3 Q12,29 17.7,25.85 Z" />
        </clipPath>
      </defs>
      <g clip-path={`url(#${clipId})`}>
        <polygon points="50,8 88,29 50,50 12,29" fill="#8fc46b" />
        <polygon points="12,29 50,50 50,64 12,43" fill="#7cb867" />
        <polygon points="12,43 50,64 50,78 12,57" fill="#6fa855" />
        <polygon points="12,57 50,78 50,92 12,71" fill="#5f9349" />
        <polygon points="88,29 50,50 50,64 88,43" fill="#578f43" />
        <polygon points="88,43 50,64 50,78 88,57" fill="#4c7a3a" />
        <polygon points="88,57 50,78 50,92 88,71" fill="#426b32" />
      </g>
      <line x1="50" y1="31" x2="50" y2="13" stroke="#4c7a3a" stroke-width={size() <= 32 ? 7 : 4} stroke-linecap="round" />
      <path d="M50,19 C36,18 22,9 27,1 C40,4 50,13 50,19 Z" fill="#4c7a3a" />
      <path d="M50,19 C64,18 78,9 73,1 C60,4 50,13 50,19 Z" fill="#4c7a3a" />
    </svg>
  );
}
