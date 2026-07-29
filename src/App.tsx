import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import { CommandPalette } from './components/CommandPalette'
import { Lyrics } from './components/Lyrics'
import { Onboarding } from './components/Onboarding'
import { Readout } from './components/Readout'
import { SettingsView } from './components/SettingsView'
import { Transport } from './components/Transport'
import { VirtualList } from './components/VirtualList'
import { WindowControls } from './components/WindowControls'
import {
  artworkUrl,
  auth,
  formatTime,
  library,
  on,
  player,
  settings,
  type AlbumRow,
  type AuthStatus,
  type LibraryCounts,
  type PlayerState,
  type PlaylistRow,
  type SongRow,
  type SyncProgress,
} from './lib/ipc'

type View = 'songs' | 'albums' | 'playlists' | 'lyrics' | 'settings'

const ROW = 40

export default function App() {
  const [view, setView] = useState<View>('songs')
  const [songs, setSongs] = useState<SongRow[]>([])
  const [albums, setAlbums] = useState<AlbumRow[]>([])
  const [playlists, setPlaylists] = useState<PlaylistRow[]>([])
  const [counts, setCounts] = useState<LibraryCounts | null>(null)
  const [progress, setProgress] = useState<SyncProgress | null>(null)
  const [state, setState] = useState<PlayerState | null>(null)
  const [authed, setAuthed] = useState<AuthStatus | null>(null)
  const [engineOk, setEngineOk] = useState(true)
  const [problem, setProblem] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<SongRow[] | null>(null)
  const [paletteOpen, setPaletteOpen] = useState(false)
  const [onboarding, setOnboarding] = useState<boolean | null>(null)

  const reload = useCallback(async () => {
    const [s, a, p, c] = await Promise.all([
      library.songs(),
      library.albums(),
      library.playlists(),
      library.counts(),
    ])
    setSongs(s)
    setAlbums(a)
    setPlaylists(p)
    setCounts(c)
  }, [])

  useEffect(() => {
    void reload()
    void player.snapshot().then(setState)
    void auth.status().then(setAuthed)
    void settings.get().then((s) => setOnboarding(!s.onboarded))

    const subs = [
      on('player://state', setState),
      on('library://progress', setProgress),
      on('library://updated', (c) => {
        setCounts(c)
        void reload()
      }),
      on('library://failed', (f) => {
        setProgress(null)
        setProblem(
          f.needsAuth ? 'Apple Music session expired. Sign in again.' : `Sync failed: ${f.reason}`,
        )
      }),
      on('auth://authenticated', () => {
        setProblem(null)
        void auth.status().then(setAuthed)
      }),
      on('auth://login-required', () =>
        setProblem('Sign in to Apple Music in the window that just opened.'),
      ),
      on('auth://lost', () => setProblem('Apple Music session expired. Sign in again.')),
      on('engine://ready', () => {
        setEngineOk(true)
        setProblem(null)
      }),
      on('engine://unavailable', (reason) => {
        setEngineOk(false)
        setProblem(`Playback engine unavailable (${reason}). Apple may have changed their player.`)
      }),
    ]
    return () => {
      for (const s of subs) void s.then((un) => un())
    }
  }, [reload])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        setPaletteOpen((v) => !v)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const timer = useRef<number>(undefined)
  useEffect(() => {
    window.clearTimeout(timer.current)
    if (!query.trim()) {
      setResults(null)
      return
    }
    timer.current = window.setTimeout(() => {
      void library.search(query).then(setResults)
    }, 120)
    return () => window.clearTimeout(timer.current)
  }, [query])

  const shown: SongRow[] = useMemo(() => results ?? songs, [results, songs])
  const empty = counts !== null && counts.songs === 0 && counts.albums === 0

  if (onboarding) {
    return (
      <div className="relative h-full">
        <Onboarding onFinished={() => setOnboarding(false)} />
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col">
      <header
        data-tauri-drag-region
        className="flex h-11 shrink-0 items-center gap-3 border-b border-rule pr-0 pl-4 [background:var(--surface-panel)]"
      >
        <span data-tauri-drag-region className="shrink-0 text-[13px] font-semibold tracking-tight">
          capsule
        </span>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search your library"
          aria-label="Search your library"
          className="min-w-0 max-w-md flex-1 bg-ground px-2.5 py-1.5 text-xs text-ink outline-none placeholder:text-muted"
        />
        <button
          onClick={() => setPaletteOpen(true)}
          className="label shrink-0 rounded border border-rule px-1.5 py-0.5 hover:text-ink"
          title="Command palette (Ctrl+K)"
        >
          ⌘K
        </button>
        <span data-tauri-drag-region className="ml-auto shrink-0 pr-3 label">
          {authed?.authenticated ? `signed in · ${authed.storefront ?? ''}` : 'not signed in'}
        </span>
        <WindowControls />
      </header>

      <div className="flex min-h-0 flex-1">
        <nav className="flex w-44 shrink-0 flex-col gap-0.5 py-3 [background:var(--surface-panel)]">
          <div className="px-4 pt-2 pb-1.5 label">
            Library
          </div>
          <Tab on={view === 'songs' && !results} onClick={() => setView('songs')}>
            Songs {counts ? <Count n={counts.songs} /> : null}
          </Tab>
          <Tab on={view === 'albums' && !results} onClick={() => setView('albums')}>
            Albums {counts ? <Count n={counts.albums} /> : null}
          </Tab>
          <Tab on={view === 'playlists' && !results} onClick={() => setView('playlists')}>
            Playlists {counts ? <Count n={counts.playlists} /> : null}
          </Tab>

          <div className="px-4 pt-4 pb-1.5 label">Now</div>
          <Tab on={view === 'lyrics' && !results} onClick={() => setView('lyrics')}>
            Lyrics
          </Tab>
          <Tab on={view === 'settings' && !results} onClick={() => setView('settings')}>
            Settings
          </Tab>

          <div className="mt-auto px-3">
            <button
              onClick={() => void library.sync()}
              className="w-full border border-rule px-2 py-1.5 text-[11px] text-muted transition-colors hover:border-accent hover:text-ink"
            >
              {progress && !progress.done ? 'Syncing…' : 'Sync library'}
            </button>
          </div>
        </nav>

        <main className="flex min-h-0 min-w-0 flex-1 flex-col">
          {problem && (
            <div className="flex items-center gap-3 border-b border-rule bg-panel px-4 py-2.5 text-xs">
              <span className="text-accent">{problem}</span>
              {!authed?.authenticated && (
                <button
                  onClick={() => void auth.showLogin()}
                  className="border border-rule px-2 py-1 text-[11px] hover:border-accent"
                >
                  Open sign-in
                </button>
              )}
            </div>
          )}

          <div className="content-surface relative min-h-0 flex-1 overflow-hidden">
            {view === 'settings' && !results ? (
              <SettingsView />
            ) : view === 'lyrics' && !results ? (
              <Lyrics state={state} />
            ) : empty && !progress ? (
              <Empty onSync={() => void library.sync()} />
            ) : results || view === 'songs' ? (
              <VirtualList
                items={shown}
                rowHeight={ROW}
                empty={<span className="text-muted">No matches</span>}
                render={(s, i) => (
                  <SongLine song={s} index={i} onPlay={() => void library.play(shown, i)} />
                )}
              />
            ) : view === 'albums' ? (
              <VirtualList
                items={albums}
                rowHeight={ROW}
                empty={<span className="text-muted">No albums yet</span>}
                render={(a) => (
                  <AlbumLine
                    album={a}
                    onPlay={() => {
                      void library.playAlbum(a.id).catch((e) => setProblem(String(e)))
                    }}
                  />
                )}
              />
            ) : (
              <VirtualList
                items={playlists}
                rowHeight={ROW}
                empty={<span className="text-muted">No playlists yet</span>}
                render={(p) => (
                  <div
                    onDoubleClick={() =>
                      void library.playPlaylist(p.id).catch((e) => setProblem(String(e)))
                    }
                    className="flex h-10 items-center gap-3 border-b border-rule/60 px-4 hover:bg-accent/6"
                    title="Double-click to play"
                  >
                    <Art id={p.id} />
                    <span className="truncate">{p.name}</span>
                  </div>
                )}
              />
            )}
          </div>
        </main>
      </div>

      <CommandPalette
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
        onNavigate={setView}
        albums={albums}
        playlists={playlists}
      />

      <Transport state={state} />
      <Readout
        state={state}
        counts={counts}
        progress={progress}
        storefront={authed?.storefront ?? null}
        engineOk={engineOk}
      />
    </div>
  )
}

function SongLine({
  song,
  index,
  onPlay,
}: {
  song: SongRow
  index: number
  onPlay: () => void
}) {
  const playable = song.catalog_id !== null
  return (
    <div
      onDoubleClick={playable ? onPlay : undefined}
      className={`grid h-10 grid-cols-[28px_36px_1fr_1fr_52px] items-center gap-3 border-b border-rule/60 px-4 ${
        playable ? 'hover:bg-accent/6' : 'opacity-45'
      }`}
      title={playable ? 'Double-click to play' : 'No catalog id — not playable'}
    >
      <span className="data text-[10px] text-muted">
        {String(index + 1).padStart(2, '0')}
      </span>
      <Art id={song.id} />
      <span className="truncate">{song.name}</span>
      <span className="truncate text-muted">{song.artist_name}</span>
      <span className="text-right data text-[10px] text-muted">
        {formatTime(song.duration_ms)}
      </span>
    </div>
  )
}

function AlbumLine({ album, onPlay }: { album: AlbumRow; onPlay: () => void }) {
  return (
    <div
      onDoubleClick={onPlay}
      className="grid h-10 grid-cols-[36px_1fr_1fr_60px] items-center gap-3 border-b border-rule/60 px-4 hover:bg-accent/6"
      title="Double-click to play"
    >
      <Art id={album.id} />
      <span className="truncate">{album.name}</span>
      <span className="truncate text-muted">{album.artist_name}</span>
      <span className="text-right data text-[10px] text-muted">
        {album.track_count} trk
      </span>
    </div>
  )
}

function Art({ id }: { id: string }) {
  return (
    <span className="block size-7 border border-rule bg-panel">
      <img
        src={artworkUrl(id, 56)}
        alt=""
        loading="lazy"
        className="size-full object-cover"
        onError={(e) => (e.currentTarget.style.visibility = 'hidden')}
      />
    </span>
  )
}

function Tab({
  on: active,
  onClick,
  children,
}: {
  on: boolean
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      onClick={onClick}
      className={`flex items-center justify-between rounded-none px-4 py-1.5 text-left transition-colors ${
        active
          ? 'bg-accent/6 text-ink'
          : 'border-l-transparent text-muted hover:text-ink'
      }`}
    >
      {children}
    </button>
  )
}

function Count({ n }: { n: number }) {
  return <span className="data text-[10px] text-muted">{n}</span>
}

function Empty({ onSync }: { onSync: () => void }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3">
      <div className="text-muted">Your library is empty here yet.</div>
      <button
        onClick={onSync}
        className="border border-accent px-3 py-1.5 text-xs text-accent hover:bg-accent/10"
      >
        Sync from Apple Music
      </button>
    </div>
  )
}
