// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'

import { NavidromeConnect } from './NavidromeConnect'

const connect = vi.fn()
const stored = {
  source: 'navidrome',
  navidrome: { url: 'https://m.example.com', username: 'deniz' },
}
vi.mock('../lib/ipc', () => ({
  navidrome: {
    connect: (...a: unknown[]) => connect(...a),
    status: () => Promise.resolve({ configured: false, url: '', username: '', insecure: false }),
  },
  settings: {
    get: () => Promise.resolve(stored),
    set: () => Promise.resolve(),
  },
}))

beforeEach(() => connect.mockReset())
afterEach(() => cleanup())

const fill = (url: string, user: string, pass: string) => {
  fireEvent.change(screen.getByLabelText(/server url/i), { target: { value: url } })
  fireEvent.change(screen.getByLabelText(/username/i), { target: { value: user } })
  fireEvent.change(screen.getByLabelText(/password/i), { target: { value: pass } })
}

const button = () => screen.getByRole('button', { name: /connect/i }) as HTMLButtonElement

it('cannot connect without a url and username', () => {
  render(<NavidromeConnect onConnected={() => {}} />)
  expect(button().disabled).toBe(true)
})

it('masks the password field', () => {
  render(<NavidromeConnect onConnected={() => {}} />)
  expect((screen.getByLabelText(/password/i) as HTMLInputElement).type).toBe('password')
})

it('warns about plaintext http but still allows connecting', () => {
  render(<NavidromeConnect onConnected={() => {}} />)
  fill('http://nas.local:4533', 'deniz', 'sesame')
  expect(screen.getByText(/not encrypted/i)).toBeDefined()
  expect(button().disabled).toBe(false)
})

it('shows no warning for https', () => {
  render(<NavidromeConnect onConnected={() => {}} />)
  fill('https://m.example.com', 'deniz', 'sesame')
  expect(screen.queryByText(/not encrypted/i)).toBeNull()
})

it('surfaces a wrong-password error', async () => {
  connect.mockRejectedValueOnce('wrong username or password')
  render(<NavidromeConnect onConnected={() => {}} />)
  fill('https://m.example.com', 'deniz', 'wrong')
  fireEvent.click(button())
  await waitFor(() => expect(screen.getByText(/wrong username or password/i)).toBeDefined())
})

it('surfaces an unreachable server differently', async () => {
  connect.mockRejectedValueOnce('server unreachable')
  render(<NavidromeConnect onConnected={() => {}} />)
  fill('https://nope.example.com', 'deniz', 'sesame')
  fireEvent.click(button())
  await waitFor(() => expect(screen.getByText(/server unreachable/i)).toBeDefined())
})

it('calls onConnected after a successful connect', async () => {
  connect.mockResolvedValueOnce(undefined)
  const onConnected = vi.fn()
  render(<NavidromeConnect onConnected={onConnected} />)
  fill('https://m.example.com', 'deniz', 'sesame')
  fireEvent.click(button())
  await waitFor(() => expect(onConnected).toHaveBeenCalled())
  expect(connect).toHaveBeenCalledWith('https://m.example.com', 'deniz', 'sesame')
})

it('trims whitespace before sending', async () => {
  connect.mockResolvedValueOnce(undefined)
  render(<NavidromeConnect onConnected={() => {}} />)
  fill('  https://m.example.com  ', '  deniz  ', 'sesame')
  fireEvent.click(button())
  await waitFor(() =>
    expect(connect).toHaveBeenCalledWith('https://m.example.com', 'deniz', 'sesame'),
  )
})

it('prefills from existing settings', () => {
  render(
    <NavidromeConnect
      initialUrl="https://m.example.com"
      initialUsername="deniz"
      onConnected={() => {}}
    />,
  )
  expect((screen.getByLabelText(/server url/i) as HTMLInputElement).value).toBe(
    'https://m.example.com',
  )
  expect(button().disabled).toBe(false)
})
