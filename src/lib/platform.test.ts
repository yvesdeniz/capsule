import { expect, it } from 'vitest'

import { credentialStoreName, dataDirLabel } from './platform'

// Real user agents from the two webviews capsule ships against.
const WEBVIEW2 =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36'
const WEBKITGTK =
  'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15'

it('names Credential Manager on Windows', () => {
  expect(credentialStoreName(WEBVIEW2)).toBe('Windows Credential Manager')
})

it('names the keyring on Linux', () => {
  expect(credentialStoreName(WEBKITGTK)).toBe('your system keyring')
})

// An unreadable agent must not claim Windows: on Linux that would point people
// at a store their system does not have.
it('falls back to the keyring when the agent says nothing', () => {
  expect(credentialStoreName('')).toBe('your system keyring')
})

it('names the data directory per platform', () => {
  expect(dataDirLabel(WEBVIEW2)).toBe('%APPDATA%\\com.deniz.capsule')
  expect(dataDirLabel(WEBKITGTK)).toBe('~/.local/share/com.deniz.capsule')
})
