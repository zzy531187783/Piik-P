const domainMatch = typeof location === "undefined" ? null : location.pathname.match(
  /^\/d\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})(?:\/|$)/i,
);

/** Server-hosted domains live under `/d/<id>/`; the App and a single-domain Server use the root. */
export const domainId = domainMatch?.[1] ?? "";
export const channelBasePath = domainId ? `/d/${domainId}` : "";

export function channelPath(path: string): string {
  return `${channelBasePath}${path}`;
}
