export function credentialStoreName(userAgent: string): string {
  return /windows/i.test(userAgent) ? 'Windows Credential Manager' : 'your system keyring'
}

export function credentialStore(): string {
  return credentialStoreName(agent())
}

export function dataDirLabel(userAgent: string): string {
  return /windows/i.test(userAgent)
    ? '%APPDATA%\\com.deniz.capsule'
    : '~/.local/share/com.deniz.capsule'
}

export function dataDir(): string {
  return dataDirLabel(agent())
}

function agent(): string {
  return typeof navigator === 'undefined' ? '' : navigator.userAgent
}
