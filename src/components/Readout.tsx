import { useEffect, useRef, useState } from 'react'

import type { LibraryCounts, PlayerState, Source, SyncProgress } from '../lib/ipc'
import { Globe, Lock, QueueList, Stack, Waveform } from './icons'

export function Readout({
  state,
  counts,
  progress,
  storefront,
  engineOk,
  source,
}: {
  state: PlayerState | null
  counts: LibraryCounts | null
  progress: SyncProgress | null
  storefront: string | null
  engineOk: boolean
  source: Source
}) {
  // Apple and Spotify play through the hidden MusicKit webview; the rest decode
  // in Rust, where none of the engine/codec/DRM badges mean anything.
  const webview = source === 'apple' || source === 'spotify'
  const syncing = progress !== null && !progress.done
  const synced = progress ? progress.songs + progress.albums + progress.playlists : 0

  return (
    <div className="label flex flex-wrap items-center justify-center gap-x-5 gap-y-1 border-t border-rule px-4 py-2 [background:var(--surface-panel)]">
      {/* Engine, codec and DRM are facts about the Apple path. A native source
          has no webview and no DRM, and claiming otherwise was simply false. */}
      {webview ? (
        <>
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
        </>
      ) : (
        <>
          <span className="flex items-center gap-1.5">
            <span className="inline-block size-1.5" style={{ background: 'var(--color-ok)' }} />
            <span style={{ color: 'var(--color-ok)' }}>native decode</span>
          </span>
          <Field icon={<Globe size={11} />} title="Source" value={source} />
          <Field icon={<Lock size={11} />} title="DRM" value="none" />
        </>
      )}
      <Field
        icon={<Stack size={11} />}
        title="Library"
        value={counts ? `${counts.songs} · ${counts.albums} · ${counts.playlists}` : '-'}
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
