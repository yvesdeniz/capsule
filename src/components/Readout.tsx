import { useEffect, useRef, useState } from 'react'

import type { LibraryCounts, PlayerState, SyncProgress } from '../lib/ipc'
import { Globe, Lock, QueueList, Stack, Waveform } from './icons'

export function Readout({
  state,
  counts,
  progress,
  storefront,
  engineOk,
}: {
  state: PlayerState | null
  counts: LibraryCounts | null
  progress: SyncProgress | null
  storefront: string | null
  engineOk: boolean
}) {
  const syncing = progress !== null && !progress.done
  const synced = progress ? progress.songs + progress.albums + progress.playlists : 0

  return (
    <div className="label flex flex-wrap items-center justify-center gap-x-5 gap-y-1 border-t border-rule px-4 py-2 [background:var(--surface-panel)]">
      {/* These three describe the Apple playback path specifically; making
          them source-aware is still outstanding. */}
      <span className="flex items-center gap-1.5">
        <span
          className="inline-block size-1.5"
          style={{ background: engineOk ? 'var(--color-ok)' : 'var(--color-crit)' }}
        />
        <span style={{ color: engineOk ? 'var(--color-ok)' : 'var(--color-crit)' }}>
          {engineOk ? 'engine ok' : 'engine down'}
        </span>
      </span>

      <Field icon={<Waveform size={11} />} title="Codec" value="aac-lc 256" />
      <Field icon={<Lock size={11} />} title="DRM" value="widevine" />
      {storefront && <Field icon={<Globe size={11} />} title="Storefront" value={storefront} />}
      <Field
        icon={<Stack size={11} />}
        title="Library"
        value={counts ? `${counts.songs} · ${counts.albums} · ${counts.playlists}` : '—'}
      />
      {state && state.queue.length > 0 && (
        <Field
          icon={<QueueList size={11} />}
          title="Queue position"
          value={`${pad((state.index ?? 0) + 1)}/${pad(state.queue.length)}`}
        />
      )}
      {syncing && (
        <span style={{ color: 'var(--color-warn)' }}>
          syncing {progress.stage} · {synced}
        </span>
      )}
    </div>
  )
}

function Field({
  icon,
  title,
  value,
}: {
  icon: React.ReactNode
  title: string
  value: string
}) {
  const [flash, setFlash] = useState(false)
  const previous = useRef(value)

  useEffect(() => {
    if (previous.current !== value) {
      previous.current = value
      setFlash(true)
      const t = window.setTimeout(() => setFlash(false), 200)
      return () => window.clearTimeout(t)
    }
  }, [value])

  return (
    <span className="flex items-center gap-1.5" title={title}>
      <span className="text-muted">{icon}</span>
      <span className={`text-ink ${flash ? 'value-change' : ''}`}>{value}</span>
    </span>
  )
}

function pad(n: number): string {
  return String(n).padStart(2, '0')
}
