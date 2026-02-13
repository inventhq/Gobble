export function formatTimestamp(ms: number): string {
  return new Date(ms).toLocaleString();
}

export function formatDate(epoch: number): string {
  return new Date(epoch * 1000).toLocaleDateString();
}

export function formatHour(epochSecs: number): string {
  const d = new Date(epochSecs * 1000);
  return d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

export function formatNumber(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
  if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K';
  return n.toString();
}

export function truncate(s: string, max: number = 40): string {
  return s.length > max ? s.slice(0, max) + '…' : s;
}

export function timeAgo(epochSecs: number): string {
  const diff = Math.floor(Date.now() / 1000) - epochSecs;
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}
