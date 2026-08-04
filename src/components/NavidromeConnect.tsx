import { useState } from 'react'

import { NavidromeFields, useNavidromeConnect } from './fields'

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
  const { password, setPassword, busy, error, connect } = useNavidromeConnect()

  const ready = url.trim() !== '' && username.trim() !== '' && !busy

  async function submit() {
    if (!ready) return
    const { ok, settings } = await connect(url, username)
    if (!ok) return
    if (settings) {
      setUrl(settings.navidrome.url)
      setUsername(settings.navidrome.username)
    }
    onConnected()
  }

  return (
    <div className="flex h-full items-center justify-center px-6">
      <div className="w-full max-w-sm">
        <h2 className="text-[13px] text-ink">Connect to Navidrome</h2>
        <p className="mt-1 mb-4 text-[11px] leading-5 text-muted">
          Your password is verified against the server before anything is stored.
        </p>

        <NavidromeFields
          url={url}
          username={username}
          password={password}
          onUrl={setUrl}
          onUsername={setUsername}
          onPassword={setPassword}
          onSubmit={() => void submit()}
        />

        {error && <p className="mt-3 text-[11px] leading-5 text-crit">{error}</p>}

        <button
          onClick={() => void submit()}
          disabled={!ready}
          className="mt-4 w-full rounded border border-rule px-2.5 py-1.5 text-[11px] text-muted transition-colors hover:border-accent hover:text-ink disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-rule disabled:hover:text-muted"
        >
          {busy ? 'Connecting…' : 'Connect'}
        </button>
      </div>
    </div>
  )
}
