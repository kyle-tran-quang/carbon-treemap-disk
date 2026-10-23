const STORAGE_KEY = 'carbon-treemap-recent-roots';
export const MAX_RECENT_ROOTS = 5;

/** Moves `root` to the front, removing any earlier copy, and caps the list. */
export function addRecentRoot(list: readonly string[], root: string): string[] {
  return [root, ...list.filter((item) => item !== root)].slice(0, MAX_RECENT_ROOTS);
}

export function loadRecentRoots(): string[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '[]');
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((item): item is string => typeof item === 'string')
      .slice(0, MAX_RECENT_ROOTS);
  } catch {
    return [];
  }
}

export function saveRecentRoots(list: readonly string[]): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(list));
}
