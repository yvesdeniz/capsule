/**
 * Which OS credential store the Rust side actually writes to.
 *
 * `keyring` picks its backend per build target: Credential Manager on Windows,
 * Secret Service over D-Bus on Linux (gnome-keyring, KWallet). Naming the wrong
 * one sends someone looking in a place that holds nothing of theirs, so the copy
 * follows the platform instead of assuming Windows.
 *
 * The user agent is the one platform signal both webviews agree on -
 * `navigator.userAgentData` exists in WebView2 but not in WebKitGTK.
 */
export function credentialStoreName(userAgent: string): string {
  return /windows/i.test(userAgent) ? 'Windows Credential Manager' : 'your system keyring'
}

/** `credentialStoreName` for the running webview. */
export function credentialStore(): string {
  return credentialStoreName(agent())
}

/**
 * Where Tauri's `app_data_dir` lands, written the way someone would type it.
 * `%APPDATA%\<identifier>` on Windows, `$XDG_DATA_HOME` (`~/.local/share`) on
 * Linux - the same directory the README documents.
 */
export function dataDirLabel(userAgent: string): string {
  return /windows/i.test(userAgent)
    ? '%APPDATA%\\com.deniz.capsule'
    : '~/.local/share/com.deniz.capsule'
}

/** `dataDirLabel` for the running webview. */
export function dataDir(): string {
  return dataDirLabel(agent())
}

function agent(): string {
  return typeof navigator === 'undefined' ? '' : navigator.userAgent
}
