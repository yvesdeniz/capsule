import { expect, it } from 'vitest'

import { isInsecureUrl } from './navidrome'

it('flags plain http', () => {
  expect(isInsecureUrl('http://nas.local:4533')).toBe(true)
})

it('does not flag https', () => {
  expect(isInsecureUrl('https://m.example.com')).toBe(false)
})

it('ignores surrounding whitespace, as the Rust side does', () => {
  expect(isInsecureUrl('  http://nas.local  ')).toBe(true)
})

it('does not flag a bare host, which is upgraded to https before use', () => {
  expect(isInsecureUrl('m.example.com')).toBe(false)
})

it('does not flag an empty field', () => {
  expect(isInsecureUrl('')).toBe(false)
})
