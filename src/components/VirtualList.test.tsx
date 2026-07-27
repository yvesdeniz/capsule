// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'

import { VirtualList } from './VirtualList'

const ROW = 40
const VIEWPORT = 800
const OVERSCAN = 8

beforeEach(() => {
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
  Object.defineProperty(HTMLDivElement.prototype, 'clientHeight', {
    configurable: true,
    get(this: HTMLDivElement) {
      return this.className.includes('overflow-y-auto') ? VIEWPORT : 0
    },
  })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  Reflect.deleteProperty(HTMLDivElement.prototype, 'clientHeight')
})

const rows = (n: number) => Array.from({ length: n }, (_, i) => i)
const renderRow = (item: number) => <span data-testid="row">{item}</span>

const expected = Math.ceil(VIEWPORT / ROW) + OVERSCAN * 2

it('measures the viewport when items arrive after an empty first render', () => {
  const { rerender } = render(
    <VirtualList items={[]} rowHeight={ROW} render={renderRow} empty={<span>No songs</span>} />,
  )
  expect(screen.getByText('No songs')).toBeDefined()

  rerender(
    <VirtualList
      items={rows(300)}
      rowHeight={ROW}
      render={renderRow}
      empty={<span>No songs</span>}
    />,
  )

  expect(screen.getAllByTestId('row')).toHaveLength(expected)
})

it('measures the viewport when it mounts with items already present', () => {
  render(<VirtualList items={rows(300)} rowHeight={ROW} render={renderRow} />)

  expect(screen.getAllByTestId('row')).toHaveLength(expected)
})
