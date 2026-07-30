/**
 * Hand-drawn monoline icons.
 *
 * No icon library on purpose: a dependency for a dozen glyphs is wasteful, the
 * CSP blocks icon-font CDNs, and off-the-shelf sets are exactly what makes an
 * app read as generic. These match the hairline language - sharp corners (square
 * is structure), thin stroke, `currentColor` so they inherit ink/muted/state.
 *
 * Two weights: transport at 1.6px for legibility at ~16px, readout at 1.4px
 * since those render around 11px and need to stay crisp.
 */
import type { SVGProps } from 'react'

type IconProps = Omit<SVGProps<SVGSVGElement>, 'strokeWidth'> & {
  size?: number
  strokeWidth?: number
}

function Svg({ size = 16, strokeWidth = 1.6, children, ...rest }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="butt"
      strokeLinejoin="miter"
      aria-hidden="true"
      {...rest}
    >
      {children}
    </svg>
  )
}

export function Play(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M7 5 L19 12 L7 19 Z" fill="currentColor" stroke="none" />
    </Svg>
  )
}

export function Pause(p: IconProps) {
  return (
    <Svg {...p}>
      <rect x="6.5" y="5" width="3.5" height="14" fill="currentColor" stroke="none" />
      <rect x="14" y="5" width="3.5" height="14" fill="currentColor" stroke="none" />
    </Svg>
  )
}

export function Prev(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M18 5 L9 12 L18 19 Z" fill="currentColor" stroke="none" />
      <rect x="5.5" y="5" width="2.4" height="14" fill="currentColor" stroke="none" />
    </Svg>
  )
}

export function Next(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M6 5 L15 12 L6 19 Z" fill="currentColor" stroke="none" />
      <rect x="16.1" y="5" width="2.4" height="14" fill="currentColor" stroke="none" />
    </Svg>
  )
}

export function Shuffle(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M4 6 h4 l9 12 h3" />
      <path d="M17 4 l3 2 -3 2" fill="none" />
      <path d="M4 18 h4 l2.5 -3.3" />
      <path d="M14 8 l3 -2 M17 20 l3 -2 -3 -2" fill="none" />
    </Svg>
  )
}

export function Repeat(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M4 12 V9 a3 3 0 0 1 3 -3 h11" />
      <path d="M15 3 l3 3 -3 3" fill="none" />
      <path d="M20 12 v3 a3 3 0 0 1 -3 3 H6" />
      <path d="M9 21 l-3 -3 3 -3" fill="none" />
    </Svg>
  )
}

export function RepeatOne(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M4 12 V9 a3 3 0 0 1 3 -3 h11" />
      <path d="M15 3 l3 3 -3 3" fill="none" />
      <path d="M20 12 v3 a3 3 0 0 1 -3 3 H6" />
      <path d="M9 21 l-3 -3 3 -3" fill="none" />
      <path d="M11 10.5 l1.5 -1 V15" strokeWidth={1.4} />
    </Svg>
  )
}

export function Waveform(p: IconProps) {
  return (
    <Svg strokeWidth={1.4} {...p}>
      <path d="M3 12 h2 M7 8 v8 M11 4 v16 M15 7 v10 M19 10 v4 M21 12 h0.5" strokeLinecap="butt" />
    </Svg>
  )
}

export function Lock(p: IconProps) {
  return (
    <Svg strokeWidth={1.4} {...p}>
      <rect x="5" y="10.5" width="14" height="9" />
      <path d="M8 10.5 V7.5 a4 4 0 0 1 8 0 V10.5" fill="none" />
    </Svg>
  )
}

export function Globe(p: IconProps) {
  return (
    <Svg strokeWidth={1.4} {...p}>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M3.5 12 h17 M12 3.5 c3 3 3 14 0 17 c-3 -3 -3 -14 0 -17" fill="none" />
    </Svg>
  )
}

export function Stack(p: IconProps) {
  return (
    <Svg strokeWidth={1.4} {...p}>
      <path d="M4 8 L12 4 L20 8 L12 12 Z" />
      <path d="M4 12 L12 16 L20 12" fill="none" />
      <path d="M4 16 L12 20 L20 16" fill="none" />
    </Svg>
  )
}

export function QueueList(p: IconProps) {
  return (
    <Svg strokeWidth={1.4} {...p}>
      <path d="M4 7 h10 M4 12 h10 M4 17 h6" strokeLinecap="butt" />
      <path d="M17 13 v6 l4 -3 Z" fill="currentColor" stroke="none" />
    </Svg>
  )
}
