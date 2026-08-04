import { useEffect, useState } from 'react'

import { auth, dev, library, settings, type Settings } from '../lib/ipc'
import { dataDir } from '../lib/platform'
import {
  Cards,
  DiscordFields,
  GLASS,
  LastfmFields,
  SOURCES,
  SourceSetup,
  Switch,
} from './fields'

/**
 * Everything in config.toml that deserves a control, in the order people
 * actually change it. Writes on every edit - there is no Save button, because a
 * settings screen that can lose your work is worse than one that cannot.
 */
export function SettingsView() {
  const [draft, setDraft] = useState<Settings | null>(null)
  const [note, setNote] = useState<string | null>(null)

  useEffect(() => {
    void settings.get().then(setDraft)
  }, [])

  if (!draft) return null

  const edit = (patch: Partial<Settings>) => {
    const next = { ...draft, ...patch }
    setDraft(next)
    void settings.set(next)
  }

  return (
    <div className="absolute inset-0 overflow-y-auto px-8 py-8">
      <div className="mx-auto max-w-xl space-y-9">
        <Section
          title="Source"
          sub="One at a time. Switching does not delete what another source already synced."
        >
          <Cards
            options={SOURCES}
            value={draft.source}
            onChange={(source) => edit({ source })}
          />
          <div className="mt-4">
            <SourceSetup draft={draft} edit={edit} />
          </div>
        </Section>

        <Section title="Appearance" sub="Takes effect on the next launch.">
          <Cards
            options={GLASS}
            value={draft.appearance.glass}
            onChange={(glass) => edit({ appearance: { glass } })}
          />
        </Section>

        <Section
          title="Lyrics timing"
          sub="Compensates the gap between the reported position and what reaches your speakers. Calibrate by ear from the lyrics view."
        >
          <div className="flex items-center gap-3">
            <button
              onClick={() =>
                edit({ lyrics: { offset_ms: Math.max(-5000, draft.lyrics.offset_ms - 100) } })
              }
              className="rounded border border-rule px-2.5 py-1.5 text-xs text-muted hover:border-accent hover:text-ink"
            >
              −100ms
            </button>
            <span className="data min-w-[72px] text-center text-xs text-ink">
              {draft.lyrics.offset_ms === 0
                ? 'in sync'
                : `${draft.lyrics.offset_ms > 0 ? '+' : ''}${(draft.lyrics.offset_ms / 1000).toFixed(2)}s`}
            </span>
            <button
              onClick={() =>
                edit({ lyrics: { offset_ms: Math.min(5000, draft.lyrics.offset_ms + 100) } })
              }
              className="rounded border border-rule px-2.5 py-1.5 text-xs text-muted hover:border-accent hover:text-ink"
            >
              +100ms
            </button>
            {draft.lyrics.offset_ms !== 0 && (
              <button
                onClick={() => edit({ lyrics: { offset_ms: 0 } })}
                className="label px-1 text-muted hover:text-ink"
              >
                reset
              </button>
            )}
          </div>
        </Section>

        <Section title="Last.fm" sub="Optional. Register an application at last.fm/api.">
          <LastfmFields draft={draft} edit={edit} />
        </Section>

        <Section
          title="Discord Rich Presence"
          sub="Optional. Create an application at discord.com/developers."
        >
          <DiscordFields draft={draft} edit={edit} />
        </Section>

        <Section title="Library" sub={`Your data lives in ${dataDir()}.`}>
          <div className="flex flex-wrap gap-2">
            <Action
              label="Sync now"
              onClick={() => {
                void library.sync()
                setNote('Sync started.')
              }}
            />
            <Action label="Sign in again" onClick={() => void auth.showLogin()} />
            <Action
              label="Run setup again"
              onClick={() => {
                edit({ onboarded: false })
                setNote('Setup will run the next time capsule starts.')
              }}
            />
          </div>
          {note && <p className="mt-3 text-[11px] text-muted">{note}</p>}
        </Section>

        <Section
          title="Developer"
          sub="Diagnostics for filing a bug report. Off by default because it is noise otherwise."
        >
          <Switch
            label="Developer mode"
            on={draft.developer}
            onChange={(developer) => edit({ developer })}
          />
          {draft.developer && <Diagnostics />}
        </Section>
      </div>
    </div>
  )
}

/**
 * The state of the app as pasteable text.
 *
 * Exists so a bug report arrives with the facts already in it: which source,
 * which backend, what the player thinks it is doing. The server URL and every
 * credential are redacted on the Rust side.
 */
function Diagnostics() {
  const [text, setText] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    void dev.diagnostics().then(setText)
  }, [])

  const refresh = async () => {
    setCopied(false)
    setText(await dev.diagnostics())
  }

  const copy = async () => {
    const latest = await dev.diagnostics()
    setText(latest)
    await navigator.clipboard.writeText(latest)
    setCopied(true)
  }

  return (
    <div className="mt-4">
      <pre className="data max-h-56 overflow-auto rounded-md border border-rule bg-ground p-3 text-[10px] leading-5 text-muted">
        {text ?? 'Reading…'}
      </pre>
      <div className="mt-2 flex items-center gap-2">
        <Action label={copied ? 'Copied' : 'Copy diagnostics'} onClick={() => void copy()} />
        <Action label="Refresh" onClick={() => void refresh()} />
      </div>
      <p className="mt-3 text-[11px] leading-5 text-muted">
        Paste this into a bug report. Your server address and password are not included.
      </p>
    </div>
  )
}

function Section({
  title,
  sub,
  children,
}: {
  title: string
  sub?: string
  children: React.ReactNode
}) {
  return (
    <section>
      <h2 className="text-[15px] font-semibold tracking-tight">{title}</h2>
      {sub && <p className="mt-1 mb-4 text-[11px] leading-5 text-muted">{sub}</p>}
      {!sub && <div className="mb-4" />}
      {children}
    </section>
  )
}

function Action({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className="rounded border border-rule px-2.5 py-1.5 text-[11px] text-muted transition-colors hover:border-accent hover:text-ink"
    >
      {label}
    </button>
  )
}
