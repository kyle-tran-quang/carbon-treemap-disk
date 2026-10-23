const UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const;

/** Decimal units (1 KB = 1000 B), matching Finder on macOS. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '0 B';
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < UNITS.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

/** Share of `whole` taken by `part`, e.g. `12%`, `3.4%` or `<0.1%`. */
export function formatPercent(part: number, whole: number): string {
  if (!(whole > 0) || !(part > 0)) return '0%';
  const value = (part / whole) * 100;
  if (value < 0.1) return '<0.1%';
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)}%`;
}
