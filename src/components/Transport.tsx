import { artworkUrl, formatTime, player, type PlayerState } from '../lib/ipc'
import { Next, Pause, Play, Prev, Repeat, RepeatOne, Shuffle } from './icons'

export function Transport({ state }: { state: PlayerState | null }) {
  const track = state && state.index !== null ? state.queue[state.index] : undefined
  const busy = state?.status === 'loading' || state?.status === 'stalled'
  const playing = state?.status === 'playing'
  const duration = track?.duration_ms ?? 0
  const repeat = state?.repeat ?? 'off'

  return (
    <div className="grid grid-cols-[44px_1fr_auto] items-center gap-4 border-t border-rule px-4 py-3 [background:var(--surface-panel)]">
      <div className="size-11 border border-rule bg-ground">
        {track && (
          <img
            src={artworkUrl(track.id, 88)}
            alt=""
            className="size-full object-cover"
            onError={(e) => (e.currentTarget.style.visibility = 'hidden')}
          />
        )}
      </div>

      <div className="flex min-w-0 flex-col gap-1.5">
        <div className="flex items-baseline gap-2 truncate">
          <span className="truncate font-semibold tracking-[-0.01em]">
            {track?.title ?? 'Nothing queued'}
          </span>
          <span className="truncate text-muted">{track?.artist ?? ''}</span>
        </div>
        <div className="flex items-center gap-2.5">
          <span className="data w-9 shrink-0 text-right text-[10px] text-muted">
            {formatTime(state?.position_ms ?? 0)}
          </span>
          <input
            type="range"
            min={0}
            max={Math.max(duration, 1)}
            value={Math.min(state?.position_ms ?? 0, duration)}
            onChange={(e) => void player.seek(Number(e.target.value))}
            disabled={!track}
            aria-label="Seek"
            className="flex-1"
            style={
              {
                '--fill': duration > 0 ? ((state?.position_ms ?? 0) / duration) * 100 : 0,
              } as React.CSSProperties
            }
          />
          <span className="data w-9 shrink-0 text-[10px] text-muted">
            {formatTime(duration)}
          </span>
        </div>
      </div>

      <div className="flex items-center gap-1">
        <Toggle
          on={state?.shuffle ?? false}
          onClick={() => void player.toggleShuffle()}
          label="Shuffle"
        >
          <Shuffle size={15} />
        </Toggle>

        <Ctl onClick={() => void player.previous()} disabled={!track} label="Previous">
          <Prev size={17} />
        </Ctl>

        <button
          onClick={() => void player.toggle()}
          disabled={!track}
          aria-label={playing ? 'Pause' : 'Play'}
          className="mx-0.5 grid size-9 place-items-center rounded-full border border-accent text-accent transition-colors duration-[120ms] hover:bg-accent/10 disabled:opacity-30"
        >
          {busy ? (
            <span className="size-2 animate-pulse rounded-full bg-accent" />
          ) : playing ? (
            <Pause size={16} />
          ) : (
            <Play size={16} style={{ marginLeft: 1 }} />
          )}
        </button>

        <Ctl onClick={() => void player.next()} disabled={!track} label="Next">
          <Next size={17} />
        </Ctl>

        <Toggle
          on={repeat !== 'off'}
          onClick={() => void player.cycleRepeat()}
          label={`Repeat: ${repeat}`}
        >
          {repeat === 'one' ? <RepeatOne size={15} /> : <Repeat size={15} />}
        </Toggle>

        <input
          type="range"
          min={0}
          max={100}
          value={state?.volume ?? 100}
          onChange={(e) => void player.setVolume(Number(e.target.value))}
          aria-label="Volume"
          className="ml-2.5 w-20"
          style={{ '--fill': state?.volume ?? 100 } as React.CSSProperties}
        />
      </div>
    </div>
  )
}

function Ctl({
  onClick,
  disabled,
  label,
  children,
}: {
  onClick: () => void
  disabled?: boolean
  label: string
  children: React.ReactNode
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      className="grid size-8 place-items-center rounded text-muted transition-colors duration-[120ms] hover:text-ink disabled:opacity-25 disabled:hover:text-muted"
    >
      {children}
    </button>
  )
}

function Toggle({
  on,
  onClick,
  label,
  children,
}: {
  on: boolean
  onClick: () => void
  label: string
  children: React.ReactNode
}) {
  return (
    <button
      onClick={onClick}
      aria-label={label}
      aria-pressed={on}
      className={`grid size-8 place-items-center rounded transition-colors duration-[120ms] ${
        on ? 'text-accent' : 'text-muted hover:text-ink'
      }`}
    >
      {children}
    </button>
  )
}
