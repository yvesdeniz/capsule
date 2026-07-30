import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import { CommandPalette } from './components/CommandPalette'
import { Lyrics } from './components/Lyrics'
import { NavidromeConnect } from './components/NavidromeConnect'
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
  navidrome as navidromeIpc,
  on,
  player,
  settings,
  type AlbumRow,
  type AuthStatus,
  type LibraryCounts,
  type NavidromeStatus,
  type PlayerState,
  type PlaylistRow,
  type SongRow,
  type Source,
  type SyncProgress,
} from './lib/ipc'

type View = 'songs' | 'albums' | 'playlists' | 'lyrics' | 'settings'

type Detail = {
  kind: 'album' | 'playlist'
  id: string
  title: string
  subtitle?: string
}

const ROW = 40

/// Which "your login stopped working" message to show. The Apple wording is
/// wrong for a Navidrome library, and it appears on two separate events.
function authMessage(source: Source) {
  return source === 'navidrome'
    ? 'Navidrome rejected your login. Reconnect to continue.'
    : 'Apple Music session expired. Sign in again.'
}

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
  const [source, setSource] = useState<Source>('apple')
  const [ndStatus, setNdStatus] = useState<NavidromeStatus | null>(null)
  // The album or playlist currently opened.
  const [detail, setDetail] = useState<Detail | null>(null)
  // The subscription effect only re-runs on [reload], so its handlers read the
  // live source through a ref rather than a value captured at subscribe time.
  const sourceRef = useRef<Source>('apple')
  sourceRef.current = source
  const lastListRefresh = useRef(0)

  // All of this belongs to one source; carrying it across a switch leaves the
  // UI describing a library Rust has already closed.
  const applySource = useCallback((next: Source) => {
    setSource((prev) => {
      if (prev !== next) {
        setQuery('')
        setResults(null)
        setProblem(null)
        setProgress(null)
        setDetail(null)
      }
      return next
    })
    if (next === 'navidrome') void navidromeIpc.status().then(setNdStatus)
    else setNdStatus(null)
  }, [])

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
    void settings.get().then((s) => {
      setOnboarding(!s.onboarded)
      applySource(s.source)
    })

    const subs = [
      on('player://state', setState),
      on('library://progress', (p) => {
        setProgress(p)
        // library://updated only lands when the whole sync finishes, which for
        // Navidrome is one request per album. Refresh as rows arrive instead,
        // throttled so a fast source does not re-query on every batch.
        const now = Date.now()
        if (!p.done && now - lastListRefresh.current > 1000) {
          lastListRefresh.current = now
          void reload()
        }
      }),
      on('library://updated', (c) => {
        setCounts(c)
        void reload()
        // Connecting from Settings switches the source underneath us and kicks
        // off a sync. Re-read it here so the status strip and transport stop
        // describing the source we are no longer on.
        void settings.get().then((s) => applySource(s.source))
      }),
      on('library://failed', (f) => {
        setProgress(null)
        setProblem(f.needsAuth ? authMessage(sourceRef.current) : `Sync failed: ${f.reason}`)
      }),
      on('auth://authenticated', () => {
        setProblem(null)
        void auth.status().then(setAuthed)
      }),
      on('auth://login-required', () =>
        setProblem('Sign in to Apple Music in the window that just opened.'),
      ),
      on('auth://lost', () => setProblem(authMessage(sourceRef.current))),
      on('engine://ready', () => {
        setEngineOk(true)
        setProblem(null)
      }),
      on('engine://unavailable', (reason) => {
        setEngineOk(false)
        setProblem(`Playback engine unavailable (${reason}). Apple may have changed their player.`)
      }),
      on('playback://error', (reason) => setProblem(`Playback failed: ${reason}`)),
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

  // The id the queue addresses a track by. Apple and Spotify play from their
  // catalog, so a row without a catalog id genuinely cannot be played; native
  // sources use their own library id and never have a catalog id, so judging
  // them by one greys out the whole library and swallows the double-click
  // before it reaches Rust.
  const trackIdOf = useCallback(
    (s: SongRow) => (source === 'navidrome' || source === 'local' ? s.id : s.catalog_id),
    [source],
  )
  const canPlay = useCallback((s: SongRow) => trackIdOf(s) !== null, [trackIdOf])

  const nowPlayingId =
    state && state.index !== null ? (state.queue[state.index]?.id ?? null) : null
  // Apple and Spotify play through the hidden webview; everything else decodes
  // in Rust, where the engine, sign-in and storefront have no meaning.
  const webviewSource = source === 'apple' || source === 'spotify'
  const empty = counts !== null && counts.songs === 0 && counts.albums === 0

  if (onboarding) {
    return (
      <div className="relative h-full">
        <Onboarding onFinished={() => setOnboarding(false)} />
      </div>
    )
  }

  // Navidrome selected but never verified: the library would just be empty
  // with no explanation, so ask for the password instead.
  if (source === 'navidrome' && ndStatus && !ndStatus.configured) {
    return (
      <div className="relative h-full">
        <NavidromeConnect
          initialUrl={ndStatus.url}
          initialUsername={ndStatus.username}
          onConnected={() => {
            void navidromeIpc.status().then(setNdStatus)
            void reload()
          }}
        />
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col">
      <header
        data-tauri-drag-region
        className="chrome flex h-11 shrink-0 items-center gap-3 border-b border-rule/70 pr-0 pl-4"
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
          {/* Apple tokens say nothing about a Navidrome session. */}
          {webviewSource
            ? authed?.authenticated
              ? `signed in · ${authed.storefront ?? ''}`
              : 'not signed in'
            : SOURCE_NAME[source].toLowerCase()}
        </span>
        <WindowControls />
      </header>

      <div className="flex min-h-0 flex-1">
        <nav className="chrome flex w-44 shrink-0 flex-col gap-0.5 py-3">
          <div className="px-4 pt-2 pb-1.5 label">
            Library
          </div>
          <Tab on={view === 'songs' && !results} onClick={() => setView('songs')}>
            Songs {counts ? <Count n={counts.songs} /> : null}
          </Tab>
          <Tab
            on={view === 'albums' && !results}
            onClick={() => {
              setDetail(null)
              setView('albums')
            }}
          >
            Albums {counts ? <Count n={counts.albums} /> : null}
          </Tab>
          <Tab
            on={view === 'playlists' && !results}
            onClick={() => {
              setDetail(null)
              setView('playlists')
            }}
          >
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
            <div className="flex items-center gap-3 border-b border-rule/70 bg-[color-mix(in_srgb,var(--color-warn)_9%,transparent)] px-4 py-2.5 text-xs">
              <span className="text-warn">{problem}</span>
              {/* Apple's login lives in the engine webview, which native
                  sources do not have. Offering it on a Navidrome error sent
                  people to the wrong service entirely. */}
              {webviewSource && !authed?.authenticated && (
                <button
                  onClick={() => void auth.showLogin()}
                  className="rounded-md border border-rule px-2 py-1 text-[11px] transition-colors hover:border-muted hover:text-ink"
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
              <Empty source={source} onSync={() => void library.sync()} />
            ) : results || view === 'songs' ? (
              <VirtualList
                items={shown}
                rowHeight={ROW}
                empty={<span className="text-muted">No matches</span>}
                render={(s, i) => (
                  <SongLine
                    song={s}
                    index={i}
                    playable={canPlay(s)}
                    playing={nowPlayingId !== null && trackIdOf(s) === nowPlayingId}
                    onPlay={() => void library.play(shown, i)}
                  />
                )}
              />
            ) : detail ? (
              <DetailView
                detail={detail}
                canPlay={canPlay}
                nowPlayingId={nowPlayingId}
                trackIdOf={trackIdOf}
                onBack={() => setDetail(null)}
                onProblem={setProblem}
              />
            ) : view === 'albums' ? (
              <VirtualList
                items={albums}
                rowHeight={ROW}
                empty={<span className="text-muted">No albums yet</span>}
                render={(a) => (
                  <AlbumLine
                    album={a}
                    onOpen={() => setDetail({ kind: 'album', id: a.id, title: a.name, subtitle: a.artist_name })}
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
                    onClick={() => setDetail({ kind: 'playlist', id: p.id, title: p.name })}
                    onDoubleClick={() =>
                      void library.playPlaylist(p.id).catch((e) => setProblem(String(e)))
                    }
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' || e.key === ' ') {
                        e.preventDefault()
                        setDetail({ kind: 'playlist', id: p.id, title: p.name })
                      }
                    }}
                    role="button"
                    tabIndex={0}
                    className="row mx-2 flex h-10 items-center gap-3 px-2 outline-none focus-visible:ring-1 focus-visible:ring-accent"
                    title="Open, or double-click to play"
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
        canPlay={canPlay}
      />

      <Transport state={state} />
      <Readout
        state={state}
        counts={counts}
        progress={progress}
        storefront={authed?.storefront ?? null}
        engineOk={engineOk}
        source={source}
      />
    </div>
  )
}

function SongLine({
  song,
  index,
  playable,
  playing,
  onPlay,
}: {
  song: SongRow
  index: number
  playable: boolean
  playing: boolean
  onPlay: () => void
}) {
  return (
    <div
      onDoubleClick={playable ? onPlay : undefined}
      onKeyDown={(e) => {
        if (playable && (e.key === 'Enter' || e.key === ' ')) {
          e.preventDefault()
          onPlay()
        }
      }}
      // Double-click alone left the whole library unreachable without a mouse.
      role="button"
      tabIndex={playable ? 0 : -1}
      aria-disabled={!playable}
      data-playing={playing}
      className={`row mx-2 grid h-10 grid-cols-[28px_36px_1fr_1fr_52px] items-center gap-3 px-2 outline-none focus-visible:ring-1 focus-visible:ring-accent ${
        playable ? '' : 'opacity-40'
      }`}
      title={playable ? 'Play' : 'Not playable from this source'}
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

/// The tracks inside an album or playlist.
///
/// Loads on open rather than up front: a library can hold hundreds of albums,
/// and only the one being looked at needs its contents.
function DetailView({
  detail,
  canPlay,
  nowPlayingId,
  trackIdOf,
  onBack,
  onProblem,
}: {
  detail: Detail
  canPlay: (s: SongRow) => boolean
  nowPlayingId: string | null
  trackIdOf: (s: SongRow) => string | null
  onBack: () => void
  onProblem: (msg: string) => void
}) {
  const [songs, setSongs] = useState<SongRow[] | null>(null)

  useEffect(() => {
    setSongs(null)
    const load =
      detail.kind === 'album'
        ? library.albumSongs(detail.id)
        : library.playlistSongs(detail.id)
    void load.then(setSongs).catch((e) => {
      onProblem(String(e))
      setSongs([])
    })
  }, [detail.kind, detail.id, onProblem])

  const total = songs?.reduce((n, s) => n + s.duration_ms, 0) ?? 0

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-3 border-b border-rule/70 px-4 py-3">
        <button
          onClick={onBack}
          className="rounded-md border border-rule px-2 py-1 text-[11px] text-muted transition-colors hover:border-muted hover:text-ink"
        >
          Back
        </button>
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13px] text-ink">{detail.title}</div>
          {detail.subtitle && (
            <div className="truncate text-[11px] text-muted">{detail.subtitle}</div>
          )}
        </div>
        <span className="label shrink-0">
          {songs ? `${songs.length} · ${formatTime(total)}` : ''}
        </span>
        <button
          onClick={() => {
            if (songs && songs.length > 0) void library.play(songs, 0)
          }}
          disabled={!songs || songs.length === 0}
          className="rounded-md border border-rule px-2.5 py-1 text-[11px] text-muted transition-colors hover:border-accent hover:text-ink disabled:opacity-40"
        >
          Play
        </button>
      </div>

      {/* VirtualList positions itself absolutely, so it needs a positioned box
          to fill - without this it resolves against an ancestor and covers the
          header above. */}
      <div className="relative min-h-0 flex-1">
        {songs === null ? (
          <div className="flex h-full items-center justify-center text-[11px] text-muted">
            Reading…
          </div>
        ) : (
          <VirtualList
            items={songs}
            rowHeight={ROW}
            empty={<span className="text-muted">Nothing in here</span>}
            render={(s, i) => (
              <SongLine
                song={s}
                index={i}
                playable={canPlay(s)}
                playing={nowPlayingId !== null && trackIdOf(s) === nowPlayingId}
                onPlay={() => void library.play(songs, i)}
              />
            )}
          />
        )}
      </div>
    </div>
  )
}

function AlbumLine({
  album,
  onOpen,
  onPlay,
}: {
  album: AlbumRow
  onOpen: () => void
  onPlay: () => void
}) {
  return (
    <div
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onOpen()
        }
      }}
      role="button"
      tabIndex={0}
      title="Open, or double-click to play"
      onDoubleClick={onPlay}
      className="row mx-2 grid h-10 grid-cols-[36px_1fr_1fr_60px] items-center gap-3 px-2 outline-none focus-visible:ring-1 focus-visible:ring-accent"
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
      className={`mx-2 flex items-center justify-between rounded-md px-2.5 py-1.5 text-left transition-colors ${
        active
          ? 'tab-on bg-[color-mix(in_srgb,var(--color-ink)_9%,transparent)] text-ink'
          : 'text-muted hover:bg-[color-mix(in_srgb,var(--color-ink)_5%,transparent)] hover:text-ink'
      }`}
    >
      {children}
    </button>
  )
}

function Count({ n }: { n: number }) {
  return <span className="data text-[10px] text-muted">{n}</span>
}

const SOURCE_NAME: Record<Source, string> = {
  apple: 'Apple Music',
  navidrome: 'Navidrome',
  spotify: 'Spotify',
  local: 'your folders',
}

function Empty({ source, onSync }: { source: Source; onSync: () => void }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3">
      <div className="text-muted">Nothing here yet.</div>
      <button
        onClick={onSync}
        className="rounded-md border border-rule px-3 py-1.5 text-xs text-muted transition-colors hover:border-muted hover:text-ink"
      >
        Sync from {SOURCE_NAME[source]}
      </button>
    </div>
  )
}
