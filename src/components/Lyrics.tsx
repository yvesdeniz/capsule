import { useEffect, useMemo, useRef, useState } from 'react'

import { library, settings, type LyricLine, type PlayerState, type Settings } from '../lib/ipc'
import { activeItem, gapProgress, timeline } from '../lib/lyrics'

export function Lyrics({ state }: { state: PlayerState | null }) {
  const track = state?.index != null ? state.queue[state.index] : undefined
  const [lines, setLines] = useState<LyricLine[]>([])
  const [plain, setPlain] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const scroller = useRef<HTMLDivElement>(null)
  const [offset, setOffset] = useOffset()

  useEffect(() => {
    if (!track) {
      setLines([])
      setPlain(null)
      return
    }
    let cancelled = false
    setLoading(true)
    void library
      .lyrics(track.id)
      .then((r) => {
        if (cancelled) return
        setLines(r.lines)
        setPlain(r.plain)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [track?.id])

  const items = useMemo(
    () => timeline(lines, track?.duration_ms ?? 0),
    [lines, track?.duration_ms],
  )

  const position = useSmoothPosition(state) - offset
  const active = activeItem(items, position)

  useEffect(() => {
    if (active == null) return
    scroller.current
      ?.querySelector(`[data-item="${active}"]`)
      ?.scrollIntoView({ block: 'center', behavior: 'smooth' })
  }, [active])

  // Correct against the nearest line start, not the active one: a tap lands a
  // little before or after the vocal, and measuring from a line that began
  // seconds ago produces an absurd offset. Implausible taps are ignored rather
  // than saved, which is how a 5s correction got stored before.
  const tap = () => {
    const starts = items.filter((i) => i.kind === 'line').map((i) => i.at)
    if (starts.length === 0) return

    const nearest = starts.reduce((best, at) =>
      Math.abs(at - position) < Math.abs(best - position) ? at : best,
    )
    const error = position - nearest
    if (Math.abs(error) > TAP_MAX_MS) return
    setOffset(offset + error)
  }

  if (!track) return <Centered>Nothing playing.</Centered>
  if (loading && items.length === 0 && !plain) return <Centered>Looking for lyrics…</Centered>
  if (items.length === 0 && !plain) return <Centered>No lyrics for this track.</Centered>

  if (items.length === 0 && plain) {
    return (
      <div className="absolute inset-0 overflow-y-auto px-8 py-10">
        <p className="mx-auto max-w-xl whitespace-pre-wrap text-[15px] leading-8 text-muted">
          {plain}
        </p>
      </div>
    )
  }

  return (
    <>
      <div ref={scroller} className="absolute inset-0 overflow-y-auto px-8 py-[40vh]">
        <div className="mx-auto max-w-xl">
          {items.map((item, i) => (
            <div key={`${item.kind}-${item.at}`} data-item={i} className="py-1.5">
              {item.kind === 'gap' ? (
                <Dots
                  progress={i === active ? gapProgress(item, position) : 0}
                  on={i === active}
                />
              ) : (
                <p
                  className={`text-[17px] leading-8 transition-colors duration-200 ${
                    i === active ? 'font-semibold text-ink' : 'text-muted/55'
                  }`}
                >
                  {item.text}
                </p>
              )}
            </div>
          ))}
        </div>
      </div>
      <Calibration offset={offset} onTap={tap} onReset={() => setOffset(0)} />
    </>
  )
}

function Dots({ progress, on }: { progress: number; on: boolean }) {
  return (
    <div className="flex items-center gap-2 py-1" aria-hidden>
      {[0, 1, 2].map((i) => {
        const share = Math.min(1, Math.max(0, progress * 3 - i))
        return (
          <span
            key={i}
            className="size-2 rounded-full transition-all duration-300"
            style={{
              background: on ? 'var(--color-ink)' : 'var(--color-muted)',
              opacity: on ? 0.25 + share * 0.75 : 0.25,
              transform: on ? `scale(${0.85 + share * 0.35})` : 'scale(0.85)',
            }}
          />
        )
      })}
    </div>
  )
}

const OFFSET_LIMIT = 5_000
/** A tap further than this from any line start is a mistap, not a measurement. */
const TAP_MAX_MS = 2_000

function useOffset(): [number, (next: number) => void] {
  const [offset, set] = useState(0)
  const loaded = useRef<Settings | null>(null)

  useEffect(() => {
    void settings.get().then((s) => {
      loaded.current = s
      set(s.lyrics.offset_ms)
    })
  }, [])

  const update = (next: number) => {
    const clamped = Math.round(Math.max(-OFFSET_LIMIT, Math.min(OFFSET_LIMIT, next)))
    set(clamped)
    const current = loaded.current
    if (!current) return
    const merged = { ...current, lyrics: { ...current.lyrics, offset_ms: clamped } }
    loaded.current = merged
    void settings.set(merged)
  }

  return [offset, update]
}

function Calibration({
  offset,
  onTap,
  onReset,
}: {
  offset: number
  onTap: () => void
  onReset: () => void
}) {
  return (
    <div className="absolute right-4 bottom-3 z-10 flex items-center gap-2 opacity-45 transition-opacity hover:opacity-100">
      <button
        onClick={onTap}
        className="label rounded border border-rule px-2 py-1 hover:text-ink"
        title="Press exactly as a line starts, and the offset corrects itself"
      >
        Tap to sync
      </button>
      <button
        onClick={onReset}
        className="data text-[10px] text-muted hover:text-ink"
        title="Current offset — click to reset"
      >
        {offset === 0 ? 'sync' : `${offset > 0 ? '+' : ''}${(offset / 1000).toFixed(2)}s`}
      </button>
    </div>
  )
}

function useSmoothPosition(state: PlayerState | null): number {
  const anchor = useRef({ ms: 0, at: 0 })
  const [, tick] = useState(0)
  const playing = state?.status === 'playing'
  const reported = state?.position_ms ?? 0

  useEffect(() => {
    anchor.current = { ms: reported, at: performance.now() }
    tick((n) => n + 1)
  }, [reported, playing])

  useEffect(() => {
    if (!playing) return
    const id = window.setInterval(() => tick((n) => n + 1), 100)
    return () => window.clearInterval(id)
  }, [playing])

  if (!playing) return reported
  return anchor.current.ms + (performance.now() - anchor.current.at)
}

function Centered({ children }: { children: React.ReactNode }) {
  return (
    <div className="absolute inset-0 flex items-center justify-center">
      <span className="text-muted">{children}</span>
    </div>
  )
}
