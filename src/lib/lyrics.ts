/**
 * Lyric timing helpers.
 *
 * These mirror `lyrics::active_line` in Rust and run on every position tick, so
 * they live here rather than crossing IPC — a round trip per second for an
 * integer comparison would be absurd.
 */
import type { LyricLine } from './ipc'

export function activeLine(lines: LyricLine[], positionMs: number): number | null {
  const first = lines[0]
  if (!first || positionMs < first.at_ms) return null

  let lo = 0
  let hi = lines.length - 1
  let found = 0
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    const line = lines[mid]
    if (line && line.at_ms <= positionMs) {
      found = mid
      lo = mid + 1
    } else {
      hi = mid - 1
    }
  }
  return found
}

export type Item =
  | { kind: 'line'; at: number; until: number; text: string }
  | { kind: 'gap'; at: number; until: number }

export const GAP_MIN_MS = 5_000

export function timeline(lines: LyricLine[], durationMs: number): Item[] {
  if (lines.length === 0) return []

  const items: Item[] = []
  const first = lines[0]
  if (first && first.at_ms >= GAP_MIN_MS) {
    items.push({ kind: 'gap', at: 0, until: first.at_ms })
  }

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]
    if (!line) continue
    const until = lines[i + 1]?.at_ms ?? Math.max(durationMs, line.at_ms)

    if (line.text.trim() === '') {
      if (until - line.at_ms >= GAP_MIN_MS) {
        items.push({ kind: 'gap', at: line.at_ms, until })
      }
      continue
    }
    items.push({ kind: 'line', at: line.at_ms, until, text: line.text })
  }

  const last = items[items.length - 1]
  if (last && last.kind === 'line' && last.until - last.at >= GAP_MIN_MS * 2) {
    const outro = last.at + GAP_MIN_MS
    last.until = outro
    items.push({ kind: 'gap', at: outro, until: last.until + GAP_MIN_MS })
  }

  return items
}

export function activeItem(items: Item[], positionMs: number): number | null {
  if (items.length === 0) return null
  let found: number | null = null
  for (let i = 0; i < items.length; i++) {
    const item = items[i]
    if (item && item.at <= positionMs) found = i
    else break
  }
  return found
}

export function gapProgress(item: Item, positionMs: number): number {
  const span = item.until - item.at
  if (span <= 0) return 1
  return Math.min(1, Math.max(0, (positionMs - item.at) / span))
}
