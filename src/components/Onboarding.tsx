import { useEffect, useState } from 'react'

import {
  auth,
  library,
  navidrome as navidromeIpc,
  on,
  settings,
  type AuthStatus,
  type Settings,
  type SyncProgress,
} from '../lib/ipc'
import { Field, Folders, SOURCES } from './fields'
import { WindowControls } from './WindowControls'

type Step = 'welcome' | 'source' | 'setup' | 'sync' | 'lastfm' | 'discord' | 'done'

const FLOW: Step[] = ['welcome', 'source', 'setup', 'sync', 'lastfm', 'discord', 'done']

export function Onboarding({ onFinished }: { onFinished: () => void }) {
  const [step, setStep] = useState<Step>('welcome')
  const [draft, setDraft] = useState<Settings | null>(null)
  const [authed, setAuthed] = useState<AuthStatus | null>(null)
  const [progress, setProgress] = useState<SyncProgress | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const [hasLibrary, setHasLibrary] = useState(false)
  // Held only until connect succeeds, then it lives in Credential Manager.
  const [password, setPassword] = useState('')
  const [connecting, setConnecting] = useState(false)

  useEffect(() => {
    void settings.get().then(setDraft)
    void auth.status().then(setAuthed)
    void library.counts().then((c) => setHasLibrary(c.songs > 0))

    const subs = [
      on('auth://authenticated', () => {
        setProblem(null)
        void auth.status().then(setAuthed)
        setStep('sync')
        void library.sync()
      }),
      on('library://progress', setProgress),
      on('library://updated', () => setStep('lastfm')),
      on('library://failed', (f) =>
        setProblem(f.needsAuth ? 'Session expired - sign in again.' : `Sync failed: ${f.reason}`),
      ),
    ]
    return () => {
      for (const s of subs) void s.then((un) => un())
    }
  }, [])

  const edit = (patch: Partial<Settings>) => setDraft((d) => (d ? { ...d, ...patch } : d))
  const save = (extra: Partial<Settings> = {}) => {
    if (draft) void settings.set({ ...draft, ...extra })
  }

  const finish = () => {
    save({ onboarded: true })
    onFinished()
  }

  const source = draft?.source ?? 'apple'
  const chosen = SOURCES.find((s) => s.id === source)

  /// The connect command pings before it stores anything, so a bad password
  /// surfaces here rather than as an empty library later. It also persists the
  /// normalised URL, so the draft is refreshed from disk afterwards to avoid
  /// a later save writing the raw input back over it.
  const connectNavidrome = async () => {
    if (!draft) return
    setConnecting(true)
    setProblem(null)
    try {
      await navidromeIpc.connect(
        draft.navidrome.url.trim(),
        draft.navidrome.username.trim(),
        password,
      )
      setPassword('')
      setDraft(await settings.get())
      setStep('sync')
    } catch (e) {
      setProblem(typeof e === 'string' ? e : 'Could not connect')
    } finally {
      setConnecting(false)
    }
  }

  const afterSetup = () => {
    save()
    if (source === 'apple') {
      if (hasLibrary) setStep('lastfm')
      else if (authed?.authenticated) {
        setStep('sync')
        void library.sync()
      } else void auth.showLogin()
    } else if (source === 'navidrome') {
      void connectNavidrome()
    } else if (source === 'local') {
      // Nothing to sign in to: the folders are the source, so scan them now.
      setStep('sync')
      void library.sync()
    } else {
      // Spotify has no client yet; keep the settings for later.
      setStep('lastfm')
    }
  }

  return (
    <div
      data-tauri-drag-region
      className="absolute inset-0 z-50 flex flex-col [background:var(--surface-ground)]"
    >
      {/* The titlebar is not mounted during onboarding, so the window needs its
          own drag surface and controls or it cannot be moved or closed. */}
      <header
        data-tauri-drag-region
        className="flex h-11 shrink-0 items-center pr-0 pl-4 [background:var(--surface-panel)]"
      >
        <span data-tauri-drag-region className="text-[13px] font-semibold tracking-tight">
          capsule
        </span>
        <div className="ml-auto">
          <WindowControls />
        </div>
      </header>

      <div
        data-tauri-drag-region
        className="content-surface flex min-h-0 flex-1 items-center justify-center overflow-y-auto p-8"
      >
        <div className="w-[560px] max-w-[92vw]">
          <Dots step={step} />

          <div className="rounded-lg border border-rule px-8 py-9 shadow-2xl backdrop-blur-2xl [background:color-mix(in_srgb,var(--color-panel)_78%,transparent)]">
            {step === 'welcome' && (
              <Panel
                title="capsule"
                body="A fast music player for Windows. Your library is mirrored locally, so browsing never waits on the network."
                action="Get started"
                onAction={() => setStep('source')}
              />
            )}

            {step === 'source' && (
              <>
                <Head title="Where does your music live?" sub="One at a time. You can change this later in config.toml." />
                <div className="mt-5 grid grid-cols-2 gap-2">
                  {SOURCES.map((s) => (
                    <button
                      key={s.id}
                      onClick={() => edit({ source: s.id })}
                      className={`rounded border px-3.5 py-3 text-left transition-colors ${
                        source === s.id
                          ? 'border-accent bg-accent/8'
                          : 'border-rule hover:border-muted'
                      }`}
                    >
                      <span className="flex items-center gap-2 text-[13px] text-ink">
                        {s.name}
                        {!s.ready && <span className="label text-[8px]">soon</span>}
                      </span>
                      <span className="mt-0.5 block text-[11px] text-muted">{s.blurb}</span>
                    </button>
                  ))}
                </div>
                <Next onClick={() => setStep('setup')} label="Continue" />
              </>
            )}

            {step === 'setup' && draft && (
              <>
                <Head
                  title={`Set up ${chosen?.name ?? 'your source'}`}
                  sub={
                    chosen?.ready
                      ? undefined
                      : 'This source is not playable yet - your settings are saved for when it lands.'
                  }
                />

                {(source === 'apple' || source === 'spotify') && (
                  <p className="mt-4 text-[13px] leading-6 text-muted">
                    Sign-in happens on the service&apos;s own login page, in a window that opens
                    next. capsule never sees your password - only the tokens the page hands back,
                    stored in Windows Credential Manager.
                  </p>
                )}

                {source === 'navidrome' && (
                  <div className="mt-4 space-y-3">
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
                      value={password}
                      onChange={setPassword}
                      onEnter={() => void connectNavidrome()}
                    />
                    <p className="text-[11px] leading-5 text-muted">
                      Checked against your server, then kept in Windows Credential Manager - never
                      in the settings file.
                    </p>
                    {draft.navidrome.url.trim().startsWith('http://') && (
                      <p className="text-[11px] leading-5 text-warn">
                        This connection is not encrypted. Anyone on the network can read your
                        login.
                      </p>
                    )}
                  </div>
                )}

                {source === 'local' && (
                  <Folders
                    folders={draft.local.folders}
                    onChange={(folders) => edit({ local: { folders } })}
                  />
                )}

                <Next
                  onClick={afterSetup}
                  disabled={
                    source === 'navidrome' &&
                    (connecting ||
                      draft.navidrome.url.trim() === '' ||
                      draft.navidrome.username.trim() === '')
                  }
                  label={
                    source === 'apple' && !authed?.authenticated && !hasLibrary
                      ? 'Open sign-in'
                      : source === 'navidrome'
                        ? connecting
                          ? 'Connecting…'
                          : 'Connect'
                        : 'Continue'
                  }
                />
              </>
            )}

            {step === 'sync' && (
              <Panel
                title="Bringing your library across"
                body={
                  progress
                    ? `${progress.stage} - ${progress.songs} songs, ${progress.albums} albums, ${progress.playlists} playlists`
                    : 'Starting…'
                }
                waiting
              />
            )}

            {step === 'lastfm' && draft && (
              <>
                <Head
                  title="Last.fm scrobbling"
                  sub="Optional. Register an application at last.fm/api to get these."
                />
                <div className="mt-4 space-y-3">
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
                <Next
                  onClick={() => {
                    save()
                    setStep('discord')
                  }}
                  label="Continue"
                  skip={() => setStep('discord')}
                />
              </>
            )}

            {step === 'discord' && draft && (
              <>
                <Head
                  title="Discord Rich Presence"
                  sub="Optional. Create an application at discord.com/developers for the ID."
                />
                <div className="mt-4">
                  <Field
                    label="Client ID"
                    value={draft.discord.client_id}
                    onChange={(v) => edit({ discord: { ...draft.discord, client_id: v } })}
                  />
                </div>
                <Next
                  onClick={() => {
                    save()
                    setStep('done')
                  }}
                  label="Continue"
                  skip={() => setStep('done')}
                />
              </>
            )}

            {step === 'done' && (
              <Panel
                title="Ready"
                body="Double-click anything to play it. Ctrl+K opens the command palette, and lyrics live under Now."
                action="Start listening"
                onAction={finish}
              />
            )}
          </div>

          {problem && <p className="mt-4 text-center text-xs text-crit">{problem}</p>}

          {step !== 'done' && (
            <button
              onClick={finish}
              className="label mx-auto mt-6 block px-2 py-1 text-muted hover:text-ink"
            >
              Skip setup
            </button>
          )}
        </div>
      </div>
    </div>
  )
}

function Head({ title, sub }: { title: string; sub?: string }) {
  return (
    <>
      <h1 className="text-[19px] font-semibold tracking-tight">{title}</h1>
      {sub && <p className="mt-2 text-[12px] leading-5 text-muted">{sub}</p>}
    </>
  )
}

function Panel({
  title,
  body,
  action,
  onAction,
  waiting,
}: {
  title: string
  body: string
  action?: string
  onAction?: () => void
  waiting?: boolean
}) {
  return (
    <>
      <Head title={title} />
      <p className="mt-3 text-[13px] leading-6 text-muted">{body}</p>
      {action && onAction && <Next onClick={onAction} label={action} />}
      {waiting && (
        <p className="mt-5 flex items-center gap-2 text-[11px] text-muted">
          <span className="size-1.5 animate-pulse rounded-full bg-accent" />
          Working…
        </p>
      )}
    </>
  )
}

function Next({
  onClick,
  label,
  skip,
  disabled,
}: {
  onClick: () => void
  label: string
  skip?: () => void
  disabled?: boolean
}) {
  return (
    <div className="mt-6 flex items-center gap-3">
      <button
        onClick={onClick}
        disabled={disabled}
        className="rounded border border-accent px-3.5 py-1.5 text-xs text-accent transition-colors hover:bg-accent/10 disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent"
      >
        {label}
      </button>
      {skip && (
        <button onClick={skip} className="label px-1 py-1 text-muted hover:text-ink">
          Skip
        </button>
      )}
    </div>
  )
}

function Dots({ step }: { step: Step }) {
  const at = FLOW.indexOf(step)
  return (
    <div className="mb-5 flex justify-center gap-2">
      {FLOW.map((s, i) => (
        <span
          key={s}
          className="h-1 rounded-full transition-all duration-300"
          style={{
            width: i === at ? 22 : 8,
            background: i <= at ? 'var(--color-accent)' : 'var(--color-rule)',
          }}
        />
      ))}
    </div>
  )
}
