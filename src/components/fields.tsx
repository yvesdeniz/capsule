import { useEffect, useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'

import { navidrome as navidromeIpc, type NavidromeStatus, type Settings, type Source } from '../lib/ipc'

export interface SourceInfo {
  id: Source
  name: string
  blurb: string
  ready: boolean
}

export const SOURCES: SourceInfo[] = [
  { id: 'apple', name: 'Apple Music', blurb: 'Sign in with your subscription', ready: true },
  { id: 'local', name: 'Local files', blurb: 'Point at a folder on this machine', ready: false },
  { id: 'navidrome', name: 'Navidrome', blurb: 'Connect to your own server', ready: true },
  { id: 'spotify', name: 'Spotify', blurb: 'Sign in with Premium', ready: false },
]

export const GLASS = [
  { id: 'none', name: 'Solid', blurb: 'No transparency' },
  { id: 'acrylic-chrome', name: 'Acrylic', blurb: 'Blur behind the chrome' },
  { id: 'acrylic-all', name: 'Acrylic (all)', blurb: 'Blur behind everything' },
  { id: 'mica', name: 'Mica', blurb: 'Tinted from your wallpaper' },
]

export function Field({
  label,
  value,
  placeholder,
  onChange,
  type = 'text',
  onEnter,
}: {
  label: string
  value: string
  placeholder?: string
  onChange: (v: string) => void
  type?: 'text' | 'password'
  onEnter?: () => void
}) {
  return (
    <label className="block">
      <span className="label">{label}</span>
      <input
        value={value}
        type={type}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && onEnter) onEnter()
        }}
        className="mt-1 w-full rounded border border-rule bg-ground px-2.5 py-1.5 text-xs text-ink outline-none placeholder:text-muted focus:border-muted"
      />
    </label>
  )
}

/** Selectable cards, used for both the source and the window material. */
export function Cards<T extends string>({
  options,
  value,
  onChange,
}: {
  options: { id: T; name: string; blurb: string; ready?: boolean }[]
  value: T
  onChange: (id: T) => void
}) {
  return (
    <div className="grid grid-cols-2 gap-2">
      {options.map((o) => (
        <button
          key={o.id}
          onClick={() => onChange(o.id)}
          className={`rounded border px-3.5 py-3 text-left transition-colors ${
            value === o.id ? 'border-accent bg-accent/8' : 'border-rule hover:border-muted'
          }`}
        >
          <span className="flex items-center gap-2 text-[13px] text-ink">
            {o.name}
            {o.ready === false && <span className="label text-[8px]">soon</span>}
          </span>
          <span className="mt-0.5 block text-[11px] text-muted">{o.blurb}</span>
        </button>
      ))}
    </div>
  )
}

/** Folder list backed by the OS picker, with duplicates rejected. */
export function Folders({
  folders,
  onChange,
}: {
  folders: string[]
  onChange: (folders: string[]) => void
}) {
  const [failed, setFailed] = useState<string | null>(null)

  const add = async () => {
    setFailed(null)
    try {
      const picked = await open({ directory: true, multiple: true, title: 'Choose music folders' })
      if (!picked) return
      const chosen = Array.isArray(picked) ? picked : [picked]
      onChange([...folders, ...chosen.filter((p) => !folders.includes(p))])
    } catch (e) {
      setFailed(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div>
      {folders.length > 0 && (
        <ul className="mb-3 space-y-1">
          {folders.map((f) => (
            <li
              key={f}
              className="flex items-center gap-2 rounded border border-rule px-2.5 py-1.5 text-xs text-ink"
            >
              <span className="min-w-0 flex-1 truncate" title={f}>
                {f}
              </span>
              <button
                onClick={() => onChange(folders.filter((x) => x !== f))}
                className="label shrink-0 px-1 text-muted hover:text-ink"
                aria-label={`Remove ${f}`}
              >
                remove
              </button>
            </li>
          ))}
        </ul>
      )}

      <button
        onClick={() => void add()}
        className="rounded border border-rule px-2.5 py-1.5 text-[11px] text-muted transition-colors hover:border-accent hover:text-ink"
      >
        {folders.length ? 'Add another folder' : 'Choose a folder'}
      </button>

      <p className="mt-3 text-[11px] text-muted">
        Scanned for FLAC, ALAC, MP3 and AAC. Subfolders are included.
      </p>
      {failed && <p className="mt-2 text-[11px] text-crit">Could not open the picker: {failed}</p>}
    </div>
  )
}

/**
 * Server details plus the password, which is the one field that cannot live in
 * `draft`: settings are written on every keystroke here, and a password must
 * never reach `config.toml`. It goes straight to the connect command, which
 * verifies it against the server before storing it in Credential Manager.
 */
function NavidromeSetup({
  draft,
  edit,
}: {
  draft: Settings
  edit: (patch: Partial<Settings>) => void
}) {
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null)
  const [status, setStatus] = useState<NavidromeStatus | null>(null)

  useEffect(() => {
    void navidromeIpc.status().then(setStatus)
  }, [])

  const insecure = draft.navidrome.url.trim().startsWith('http://')
  const ready =
    !busy && draft.navidrome.url.trim() !== '' && draft.navidrome.username.trim() !== ''

  const connect = async () => {
    if (!ready) return
    setBusy(true)
    setResult(null)
    try {
      await navidromeIpc.connect(
        draft.navidrome.url.trim(),
        draft.navidrome.username.trim(),
        password,
      )
      setPassword('')
      setResult({ ok: true, text: 'Connected. Syncing your library…' })
      void navidromeIpc.status().then(setStatus)
    } catch (e) {
      setResult({ ok: false, text: typeof e === 'string' ? e : 'Could not connect' })
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-3">
      <Field
        label="Server URL"
        placeholder="https://music.example.com"
        value={draft.navidrome.url}
        onChange={(v) => edit({ navidrome: { ...draft.navidrome, url: v } })}
      />
      <Field
        label="Username"
        value={draft.navidrome.username}
        onChange={(v) => edit({ navidrome: { ...draft.navidrome, username: v } })}
      />
      <Field
        label="Password"
        type="password"
        placeholder={status?.configured ? 'Stored — enter to change' : ''}
        value={password}
        onChange={setPassword}
        onEnter={() => void connect()}
      />

      <div className="flex items-center gap-3 pt-1">
        <button
          onClick={() => void connect()}
          disabled={!ready}
          className="rounded border border-rule px-2.5 py-1.5 text-[11px] text-muted transition-colors hover:border-accent hover:text-ink disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-rule disabled:hover:text-muted"
        >
          {busy ? 'Connecting…' : status?.configured ? 'Reconnect' : 'Connect'}
        </button>
        {status?.configured && !result && (
          <span className="label text-muted">connected as {status.username}</span>
        )}
      </div>

      {result && (
        <p
          className="text-[11px] leading-5"
          style={{ color: result.ok ? 'var(--color-ok)' : 'var(--color-crit)' }}
        >
          {result.text}
        </p>
      )}
      {insecure && (
        <p className="text-[11px] leading-5 text-warn">
          This connection is not encrypted. Anyone on the network can read your login.
        </p>
      )}
      <p className="text-[11px] leading-5 text-muted">
        The password is checked against your server, then kept in Windows Credential Manager —
        never in the settings file.
      </p>
    </div>
  )
}

/** The connection details for whichever source is selected. */
export function SourceSetup({
  draft,
  edit,
}: {
  draft: Settings
  edit: (patch: Partial<Settings>) => void
}) {
  if (draft.source === 'apple' || draft.source === 'spotify') {
    return (
      <p className="text-[13px] leading-6 text-muted">
        Sign-in happens on the service&apos;s own login page. capsule never sees your password —
        only the tokens the page hands back, stored in Windows Credential Manager.
      </p>
    )
  }

  if (draft.source === 'navidrome') {
    return <NavidromeSetup draft={draft} edit={edit} />
  }

  return (
    <Folders folders={draft.local.folders} onChange={(folders) => edit({ local: { folders } })} />
  )
}

export function LastfmFields({
  draft,
  edit,
}: {
  draft: Settings
  edit: (patch: Partial<Settings>) => void
}) {
  return (
    <div className="space-y-3">
      <Field
        label="API key"
        value={draft.lastfm.api_key}
        onChange={(v) => edit({ lastfm: { ...draft.lastfm, api_key: v } })}
      />
      <Field
        label="Shared secret"
        value={draft.lastfm.shared_secret}
        onChange={(v) => edit({ lastfm: { ...draft.lastfm, shared_secret: v } })}
      />
    </div>
  )
}

export function DiscordFields({
  draft,
  edit,
}: {
  draft: Settings
  edit: (patch: Partial<Settings>) => void
}) {
  return (
    <Field
      label="Client ID"
      value={draft.discord.client_id}
      onChange={(v) => edit({ discord: { client_id: v } })}
    />
  )
}
