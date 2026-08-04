export function isInsecureUrl(url: string): boolean {
  return url.trim().startsWith('http://')
}
