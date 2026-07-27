fn main() {
    // Declaring commands here is what makes Tauri generate the `allow-<command>`
    // permissions. Without it the commands exist but cannot be granted, and the
    // remote engine webview fails with "<cmd> not allowed. Plugin not found".
    // Local origins get these implicitly; remote origins do not.
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&[
                // called by the engine hook running on music.apple.com
                "engine_ready",
                "engine_tokens",
                "engine_event",
                "engine_log",
                // called by our own UI
                "player_snapshot",
                "player_play",
                "player_pause",
                "player_toggle",
                "player_next",
                "player_previous",
                "player_seek",
                "player_set_volume",
                "player_toggle_shuffle",
                "player_cycle_repeat",
                "auth_status",
                "auth_show_login",
                "dev_load_recent",
                "library_songs",
                "library_albums",
                "library_playlists",
                "library_album_songs",
                "library_search",
                "library_counts",
                "library_sync",
                "lyrics_for",
                "settings_get",
                "settings_set",
                "play_songs",
                "play_album",
                "play_playlist",
            ]),
        ),
    )
    .expect("failed to run tauri-build");
}
