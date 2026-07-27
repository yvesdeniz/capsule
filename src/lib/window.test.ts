import { describe, expect, it } from 'vitest'

import { visibleRange } from './window'

const ROW = 40

describe('visibleRange', () => {
  it('mounts only a window, not the whole library', () => {
    const r = visibleRange(0, 800, ROW, 50_000, 8)
    expect(r.first).toBe(0)
    expect(r.last).toBe(36)
    expect(r.totalHeight).toBe(50_000 * ROW)
  })

  it('offsets the slice so rows land at the right scroll position', () => {
    const r = visibleRange(4000, 800, ROW, 1000, 8)
    expect(r.first).toBe(100 - 8)
    expect(r.offsetY).toBe(r.first * ROW)
  })

  it('never returns a negative first index near the top', () => {
    const r = visibleRange(40, 800, ROW, 1000, 8)
    expect(r.first).toBe(0)
    expect(r.offsetY).toBe(0)
  })

  it('clamps the last index at the end of the list', () => {
    const r = visibleRange(1000 * ROW, 800, ROW, 1000, 8)
    expect(r.last).toBe(1000)
    expect(r.first).toBeLessThanOrEqual(r.last)
  })

  it('handles an empty list without producing a range', () => {
    const r = visibleRange(0, 800, ROW, 0)
    expect(r).toEqual({ first: 0, last: 0, totalHeight: 0, offsetY: 0 })
  })

  it('refuses a zero row height instead of dividing by zero', () => {
    const r = visibleRange(0, 800, 0, 5000)
    expect(r.last).toBe(0)
    expect(Number.isFinite(r.totalHeight)).toBe(true)
  })

  it('tolerates a negative scrollTop from overscroll bounce', () => {
    const r = visibleRange(-200, 800, ROW, 1000)
    expect(r.first).toBe(0)
  })

  it('handles a viewport taller than the content', () => {
    const r = visibleRange(0, 5000, ROW, 10)
    expect(r.last).toBe(10)
  })

  it('always covers the visible area even with zero overscan', () => {
    const r = visibleRange(0, 800, ROW, 1000, 0)
    expect(r.last - r.first).toBeGreaterThanOrEqual(20)
  })
})
