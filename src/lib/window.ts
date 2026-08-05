export interface Range {
  first: number
  last: number
  totalHeight: number
  offsetY: number
}

export function visibleRange(
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  count: number,
  overscan = 8,
): Range {
  if (rowHeight <= 0 || count <= 0) {
    return { first: 0, last: 0, totalHeight: 0, offsetY: 0 }
  }

  const safeScroll = Math.max(0, scrollTop)
  const first = Math.max(0, Math.floor(safeScroll / rowHeight) - overscan)
  const visible = Math.ceil(Math.max(0, viewportHeight) / rowHeight) + overscan * 2
  const last = Math.min(count, first + visible)

  return {
    first,
    last,
    totalHeight: count * rowHeight,
    offsetY: first * rowHeight,
  }
}
