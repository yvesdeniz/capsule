import { useEffect, useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'

import {
  lastfm as lastfmIpc,
  navidrome as navidromeIpc,
  on,
  settings as settingsIpc,
  type LastfmStatus,
  type NavidromeStatus,
  type Settings,
  type Source,
} from '../lib/ipc'
import { isInsecureUrl } from '../lib/navidrome'
import { credentialStore } from '../lib/platform'

export interface SourceInfo {
  id: Source
  name: string
  blurb: string
  ready: boolean
}

export const SOURCES: SourceInfo[] = [
  { id: 'apple', name: 'Apple Music', blurb: 'Sign in with your subscription', ready: true },
  { id: 'local', name: 'Local files', blurb: 'Point at a folder on this machine', ready: true },
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

export interface NavidromeConnectResult {
  ok: boolean
  settings: Settings | null
}

export function useNavidromeConnect() {
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const connect = async (url: string, username: string): Promise<NavidromeConnectResult> => {
    if (busy) return { ok: false, settings: null }
    setBusy(true)
    setError(null)
    try {
      await navidromeIpc.connect(url.trim(), username.trim(), password)
      setPassword('')
    } catch (e) {
      setError(typeof e === 'string' ? e : 'Could not connect')
      return { ok: false, settings: null }
    } finally {
      setBusy(false)
    }

    try {
      return { ok: true, settings: await settingsIpc.get() }
    } catch {
      return { ok: true, settings: null }
    }
  }

  return { password, setPassword, busy, error, setError, connect }
}

export function NavidromeFields({
  url,
  username,
  password,
  onUrl,
  onUsername,
  onPassword,
  onSubmit,
  passwordPlaceholder,
}: {
  url: string
  username: string
  password: string
  onUrl: (v: string) => void
  onUsername: (v: string) => void
  onPassword: (v: string) => void
  onSubmit: () => void
  passwordPlaceholder?: string
}) {
  return (
    <div className="space-y-3">
      <Field
        label="Server URL"
        placeholder="https://music.example.com"
        value={url}
        onChange={onUrl}
        onEnter={onSubmit}
      />
      <Field label="Username" value={username} onChange={onUsername} onEnter={onSubmit} />
      <Field
        label="Password"
        type="password"
        placeholder={passwordPlaceholder}
        value={password}
        onChange={onPassword}
        onEnter={onSubmit}
      />
      {isInsecureUrl(url) && (
        <p className="text-[11px] leading-5 text-warn">
          This connection is not encrypted. Anyone on the network can read your login.
        </p>
      )}
      <p className="text-[11px] leading-5 text-muted">
        The password is checked against your server, then kept in {credentialStore()} - never in
        the settings file.
      </p>
    </div>
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
 * verifies it against the server before storing it in the OS credential store.
 */
function NavidromeSetup({
  draft,
  edit,
}: {
  draft: Settings
  edit: (patch: Partial<Settings>) => void
}) {
  const { password, setPassword, busy, error, setError, connect } = useNavidromeConnect()
  const [done, setDone] = useState(false)
  const [status, setStatus] = useState<NavidromeStatus | null>(null)

  useEffect(() => {
    void navidromeIpc.status().then(setStatus)
  }, [])

  const ready =
    !busy && draft.navidrome.url.trim() !== '' && draft.navidrome.username.trim() !== ''

  const submit = async () => {
    if (!ready) return
    setDone(false)
    const { ok, settings } = await connect(draft.navidrome.url, draft.navidrome.username)
    if (!ok) return
    if (settings) edit({ navidrome: settings.navidrome })
    setDone(true)
    void navidromeIpc.status().then(setStatus)
  }

  return (
    <div className="space-y-3">
      <NavidromeFields
        url={draft.navidrome.url}
        username={draft.navidrome.username}
        password={password}
        onUrl={(v) => {
          setError(null)
          setDone(false)
          edit({ navidrome: { ...draft.navidrome, url: v } })
        }}
        onUsername={(v) => {
          setError(null)
          setDone(false)
          edit({ navidrome: { ...draft.navidrome, username: v } })
        }}
        onPassword={setPassword}
        onSubmit={() => void submit()}
        passwordPlaceholder={status?.configured ? 'Stored - enter to change' : ''}
      />

      <div className="flex items-center gap-3 pt-1">
        <button
          onClick={() => void submit()}
          disabled={!ready}
          className="rounded border border-rule px-2.5 py-1.5 text-[11px] text-muted transition-colors hover:border-accent hover:text-ink disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-rule disabled:hover:text-muted"
        >
          {busy ? 'Connecting…' : status?.configured ? 'Reconnect' : 'Connect'}
        </button>
        {status?.configured && !done && !error && (
          <span className="label text-muted">connected as {status.username}</span>
        )}
      </div>

      {done && (
        <p className="text-[11px] leading-5" style={{ color: 'var(--color-ok)' }}>
          Connected. Syncing your library…
        </p>
      )}
      {error && (
        <p className="text-[11px] leading-5" style={{ color: 'var(--color-crit)' }}>
          {error}
        </p>
      )}
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
        Sign-in happens on the service&apos;s own login page. capsule never sees your password -
        only the tokens the page hands back, stored in {credentialStore()}.
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
      <LastfmAccount />
    </div>
  )
}

function LastfmAccount() {
  const [status, setStatus] = useState<LastfmStatus | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [waiting, setWaiting] = useState(false)

  useEffect(() => {
    void lastfmIpc.status().then(setStatus)
    const subs = [
      on('lastfm://linked', (s) => {
        setStatus(s)
        setWaiting(false)
        setNote(null)
      }),
      on('lastfm://failed', (reason) => {
        setWaiting(false)
        setNote(reason)
      }),
    ]
    return () => {
      for (const s of subs) void s.then((un) => un())
    }
  }, [])

  const link = async () => {
    setNote(null)
    setWaiting(true)
    try {
      await lastfmIpc.connect()
    } catch (e) {
      setWaiting(false)
      setNote(typeof e === 'string' ? e : 'Could not reach Last.fm')
    }
  }

  const unlink = async () => {
    setNote(null)
    try {
      await lastfmIpc.disconnect()
    } catch (e) {
      setNote(typeof e === 'string' ? e : 'Could not sign out')
    }
  }

  if (!status) return null

  return (
    <div className="space-y-2 pt-1">
      <div className="flex items-center gap-3">
        <button
          onClick={() => void (status.linked ? unlink() : link())}
          disabled={!status.configured || waiting}
          className="rounded border border-rule px-2.5 py-1.5 text-[11px] text-muted transition-colors hover:border-accent hover:text-ink disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-rule disabled:hover:text-muted"
        >
          {status.linked ? 'Sign out' : waiting ? 'Waiting…' : 'Connect account'}
        </button>
        {status.linked && status.username && (
          <span className="label text-muted">scrobbling as {status.username}</span>
        )}
      </div>
      <p className="text-[11px] leading-5 text-muted">
        {!status.configured
          ? 'Add a key and secret above, then connect your account.'
          : status.linked
            ? 'Plays are sent straight to Last.fm, so local files count too.'
            : waiting
              ? 'Approve capsule in the window that just opened.'
              : 'Without this, only a server that scrobbles for you can report plays.'}
      </p>
      {note && <p className="text-[11px] leading-5 text-crit">{note}</p>}
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
    <div className="space-y-4">
      <Field
        label="Client ID"
        value={draft.discord.client_id}
        onChange={(v) => edit({ discord: { ...draft.discord, client_id: v } })}
      />
      <Switch
        label="Serve cover art from your server"
        note="Discord fetches the image itself, so your server must be reachable from the internet. The link it receives also works for the rest of your library - leave this off unless you are comfortable with that."
        on={draft.discord.serve_art_from_server}
        onChange={(v) => edit({ discord: { ...draft.discord, serve_art_from_server: v } })}
      />
      {!draft.discord.serve_art_from_server && (
        <p className="text-[11px] leading-5 text-muted">
          Cover art comes from Last.fm instead, which needs the API key above and only finds
          albums Last.fm knows.
        </p>
      )}
    </div>
  )
}

export function Switch({
  label,
  note,
  on,
  onChange,
}: {
  label: string
  note?: string
  on: boolean
  onChange: (v: boolean) => void
}) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div className="min-w-0">
        <div className="text-[13px] text-ink">{label}</div>
        {note && <p className="mt-0.5 text-[11px] leading-5 text-muted">{note}</p>}
      </div>
      <button
        role="switch"
        aria-checked={on}
        aria-label={label}
        onClick={() => onChange(!on)}
        className={`mt-0.5 h-5 w-9 shrink-0 rounded-full border transition-colors ${
          on ? 'border-accent bg-accent/30' : 'border-rule bg-ground'
        }`}
      >
        <span
          className={`block size-3.5 rounded-full transition-transform ${
            on ? 'translate-x-4 bg-accent' : 'translate-x-0.5 bg-muted'
          }`}
        />
      </button>
    </div>
  )
}
