import { Command } from 'cmdk'
import { useEffect, useRef, useState } from 'react'

import {
  artworkUrl,
  formatTime,
  library,
  player,
  type AlbumRow,
  type PlaylistRow,
  type SongRow,
} from '../lib/ipc'
import { Next, Pause, Play, Prev, Repeat, Shuffle, Stack } from './icons'

type Nav = 'songs' | 'albums' | 'playlists'

export function CommandPalette({
  open,
  onOpenChange,
  onNavigate,
  albums,
  playlists,
}: {
  open: boolean
  onOpenChange: (v: boolean) => void
  onNavigate: (view: Nav) => void
  albums: AlbumRow[]
  playlists: PlaylistRow[]
}) {
  const [query, setQuery] = useState('')
  const [songs, setSongs] = useState<SongRow[]>([])
  const timer = useRef<number>(undefined)

  useEffect(() => {
    if (open) {
      setQuery('')
      setSongs([])
    }
  }, [open])

  useEffect(() => {
    window.clearTimeout(timer.current)
    if (!query.trim()) {
      setSongs([])
      return
    }
    timer.current = window.setTimeout(() => {
      void library.search(query, 8).then(setSongs)
    }, 90)
    return () => window.clearTimeout(timer.current)
  }, [query])

  const run = (fn: () => void) => {
    fn()
    onOpenChange(false)
  }

  const q = query.trim().toLowerCase()
  const matchedAlbums = q
    ? albums
        .filter((a) => `${a.name} ${a.artist_name}`.toLowerCase().includes(q))
        .slice(0, 5)
    : []
  const matchedPlaylists = q
    ? playlists.filter((p) => p.name.toLowerCase().includes(q)).slice(0, 5)
    : []

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/30 pt-[12vh] backdrop-blur-sm"
      onClick={() => onOpenChange(false)}
    >
      <Command
        shouldFilter={false}
        loop
        onClick={(e) => e.stopPropagation()}
        className="flex max-h-[62vh] w-[560px] max-w-[92vw] flex-col overflow-hidden border border-rule shadow-2xl backdrop-blur-2xl [background:color-mix(in_srgb,var(--color-panel)_78%,transparent)]"
      >
        <Command.Input
          value={query}
          onValueChange={setQuery}
          autoFocus
          placeholder="Search songs, albums, playlists, or run a command…"
          className="bg-transparent px-4 py-3 text-sm text-ink outline-none placeholder:text-muted"
        />
        <Command.List className="flex-1 overflow-y-auto p-1.5">
          <Command.Empty className="px-3 py-6 text-center text-xs text-muted">
            No results.
          </Command.Empty>

          {songs.length > 0 && (
            <Group label="Songs">
              {songs.map((s) => (
                <Item
                  key={`song-${s.id}`}
                  value={`song-${s.id}`}
                  disabled={!s.catalog_id}
                  onSelect={() => run(() => void library.play([s], 0))}
                >
                  <img
                    src={artworkUrl(s.id, 40)}
                    alt=""
                    className="size-6 shrink-0 border border-rule object-cover"
                    onError={(e) => (e.currentTarget.style.visibility = 'hidden')}
                  />
                  <span className="min-w-0 flex-1 truncate">{s.name}</span>
                  <span className="shrink-0 truncate text-muted">{s.artist_name}</span>
                  <span className="data shrink-0 text-[10px] text-muted">
                    {formatTime(s.duration_ms)}
                  </span>
                </Item>
              ))}
            </Group>
          )}

          {matchedAlbums.length > 0 && (
            <Group label="Albums">
              {matchedAlbums.map((a) => (
                <Item
                  key={`album-${a.id}`}
                  value={`album-${a.id}`}
                  onSelect={() => run(() => void library.playAlbum(a.id))}
                >
                  <img
                    src={artworkUrl(a.id, 40)}
                    alt=""
                    className="size-6 shrink-0 border border-rule object-cover"
                    onError={(e) => (e.currentTarget.style.visibility = 'hidden')}
                  />
                  <span className="min-w-0 flex-1 truncate">{a.name}</span>
                  <span className="shrink-0 truncate text-muted">{a.artist_name}</span>
                </Item>
              ))}
            </Group>
          )}

          {matchedPlaylists.length > 0 && (
            <Group label="Playlists">
              {matchedPlaylists.map((p) => (
                <Item
                  key={`pl-${p.id}`}
                  value={`pl-${p.id}`}
                  onSelect={() => run(() => void library.playPlaylist(p.id))}
                >
                  <Stack size={16} />
                  <span className="min-w-0 flex-1 truncate">{p.name}</span>
                </Item>
              ))}
            </Group>
          )}

          <Group label="Playback">
            <Item value="cmd play/pause toggle" onSelect={() => run(() => void player.toggle())}>
              <Play size={15} />
              Play / Pause
            </Item>
            <Item value="cmd next skip forward" onSelect={() => run(() => void player.next())}>
              <Next size={15} />
              Next track
            </Item>
            <Item value="cmd previous skip back" onSelect={() => run(() => void player.previous())}>
              <Prev size={15} />
              Previous track
            </Item>
            <Item value="cmd shuffle" onSelect={() => run(() => void player.toggleShuffle())}>
              <Shuffle size={15} />
              Toggle shuffle
            </Item>
            <Item value="cmd repeat" onSelect={() => run(() => void player.cycleRepeat())}>
              <Repeat size={15} />
              Cycle repeat
            </Item>
            <Item value="cmd pause stop" onSelect={() => run(() => void player.pause())}>
              <Pause size={15} />
              Pause
            </Item>
          </Group>

          <Group label="Go to">
            <Item value="go songs" onSelect={() => run(() => onNavigate('songs'))}>
              Songs
            </Item>
            <Item value="go albums" onSelect={() => run(() => onNavigate('albums'))}>
              Albums
            </Item>
            <Item value="go playlists" onSelect={() => run(() => onNavigate('playlists'))}>
              Playlists
            </Item>
            <Item value="cmd sync refresh library" onSelect={() => run(() => void library.sync())}>
              Sync library
            </Item>
          </Group>
        </Command.List>

        <div className="label flex items-center gap-3 border-t border-rule px-3 py-1.5">
          <span>↑↓ navigate</span>
          <span>↵ select</span>
          <span>esc close</span>
        </div>
      </Command>
    </div>
  )
}

function Group({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <Command.Group
      heading={label}
      className="[&_[cmdk-group-heading]]:label [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:pt-2 [&_[cmdk-group-heading]]:pb-1"
    >
      {children}
    </Command.Group>
  )
}

function Item({
  value,
  onSelect,
  disabled,
  children,
}: {
  value: string
  onSelect: () => void
  disabled?: boolean
  children: React.ReactNode
}) {
  return (
    <Command.Item
      value={value}
      disabled={disabled}
      onSelect={onSelect}
      className="flex cursor-pointer items-center gap-2.5 rounded px-2.5 py-2 text-sm text-ink data-[disabled=true]:opacity-35 data-[selected=true]:bg-accent/12 data-[selected=true]:text-ink"
    >
      {children}
    </Command.Item>
  )
}
