import { useState } from 'react'

import { navidrome } from '../lib/ipc'
import { Field } from './fields'

/**
 * Shown when the source is navidrome but no verified credential exists. The
 * password only travels as far as the Rust side, which pings the server
 * before storing anything in Windows Credential Manager.
 */
export function NavidromeConnect({
  initialUrl = '',
  initialUsername = '',
  onConnected,
}: {
  initialUrl?: string
  initialUsername?: string
  onConnected: () => void
}) {
  const [url, setUrl] = useState(initialUrl)
  const [username, setUsername] = useState(initialUsername)
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const insecure = url.trim().startsWith('http://')
  const ready = url.trim() !== '' && username.trim() !== '' && !busy

  async function connect() {
    if (!ready) return
    setBusy(true)
    setError(null)
    try {
      await navidrome.connect(url.trim(), username.trim(), password)
      onConnected()
    } catch (e) {
      setError(typeof e === 'string' ? e : 'Could not connect')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex h-full items-center justify-center px-6">
      <div className="w-full max-w-sm">
        <h2 className="text-[13px] text-ink">Connect to Navidrome</h2>
        <p className="mt-1 mb-4 text-[11px] leading-5 text-muted">
          Your password is verified against the server, then stored in Windows Credential Manager
          - never in the settings file.
        </p>

        <div className="space-y-3">
          <Field
            label="Server URL"
            placeholder="https://music.example.com"
            value={url}
            onChange={setUrl}
            onEnter={() => void connect()}
          />
          <Field
            label="Username"
            value={username}
            onChange={setUsername}
            onEnter={() => void connect()}
          />
          <Field
            label="Password"
            type="password"
            value={password}
            onChange={setPassword}
            onEnter={() => void connect()}
          />
        </div>

        {insecure && (
          <p className="mt-3 text-[11px] leading-5 text-warn">
            This connection is not encrypted. Anyone on the network can read your login.
          </p>
        )}
        {error && <p className="mt-3 text-[11px] leading-5 text-crit">{error}</p>}

        <button
          onClick={() => void connect()}
          disabled={!ready}
          className="mt-4 w-full rounded border border-rule px-2.5 py-1.5 text-[11px] text-muted transition-colors hover:border-accent hover:text-ink disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-rule disabled:hover:text-muted"
        >
          {busy ? 'Connecting…' : 'Connect'}
        </button>
      </div>
    </div>
  )
}
