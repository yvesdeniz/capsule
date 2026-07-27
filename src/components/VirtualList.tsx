import { useCallback, useState } from 'react'

import { visibleRange } from '../lib/window'

export function VirtualList<T>({
  items,
  rowHeight,
  render,
  overscan = 8,
  empty,
}: {
  items: T[]
  rowHeight: number
  render: (item: T, index: number) => React.ReactNode
  overscan?: number
  empty?: React.ReactNode
}) {
  const [scrollTop, setScrollTop] = useState(0)
  const [height, setHeight] = useState(0)

  const ref = useCallback((el: HTMLDivElement | null) => {
    if (!el) return
    const measure = () => setHeight(el.clientHeight)
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    measure()
    requestAnimationFrame(measure)
    return () => ro.disconnect()
  }, [])

  if (items.length === 0 && empty) {
    return <div className="absolute inset-0 flex items-center justify-center">{empty}</div>
  }

  const { first, last, totalHeight, offsetY } = visibleRange(
    scrollTop,
    height,
    rowHeight,
    items.length,
    overscan,
  )
  const slice = items.slice(first, last)

  return (
    <div
      ref={ref}
      onScroll={(e) => {
        setScrollTop(e.currentTarget.scrollTop)
        const h = e.currentTarget.clientHeight
        if (h > 0 && h !== height) setHeight(h)
      }}
      className="absolute inset-0 overflow-y-auto"
    >
      <div style={{ height: totalHeight, position: 'relative' }}>
        <div style={{ transform: `translateY(${offsetY}px)` }}>
          {slice.map((item, i) => (
            <div key={first + i} style={{ height: rowHeight }}>
              {render(item, first + i)}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
