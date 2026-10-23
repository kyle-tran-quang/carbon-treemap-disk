import type { ViewKind } from '../ipc';

export interface SelectedItem {
  id: number;
  name: string;
  path: string;
  kind: 'dir' | 'file';
  size: number;
  files: number;
}

export interface SelectionSummary {
  /** Items the user selected. */
  count: number;
  /** Bytes, counting an item once even when an ancestor is also selected. */
  size: number;
}

interface Selectable {
  id?: number | null;
  name: string;
  path: string;
  kind: ViewKind;
  size: number;
  files: number;
}

/** Only real directories and files can be selected. */
export function toSelectedItem(source: Selectable): SelectedItem | null {
  const { id, kind } = source;
  if (id === null || id === undefined) return null;
  if (kind !== 'dir' && kind !== 'file') return null;
  return {
    id,
    kind,
    name: source.name,
    path: source.path,
    size: source.size,
    files: source.files,
  };
}

/** Plain click replaces the selection; a modifier click toggles one item. */
export function selectItem(
  items: readonly SelectedItem[],
  item: SelectedItem,
  additive: boolean,
): SelectedItem[] {
  const present = items.some((existing) => existing.id === item.id);
  if (!additive) return [item];
  return present ? items.filter((existing) => existing.id !== item.id) : [...items, item];
}

/** Adds (`on`) or removes (`!on`) every item of `subset`. */
export function setItems(
  items: readonly SelectedItem[],
  subset: readonly SelectedItem[],
  on: boolean,
): SelectedItem[] {
  const ids = new Set(subset.map((item) => item.id));
  const kept = items.filter((item) => !ids.has(item.id));
  return on ? [...kept, ...subset] : kept;
}

function withSlash(path: string): string {
  return path.endsWith('/') ? path : `${path}/`;
}

export function summarize(items: readonly SelectedItem[]): SelectionSummary {
  const size = items
    .filter(
      (item) =>
        !items.some(
          (other) => other !== item && item.path.startsWith(withSlash(other.path)),
        ),
    )
    .reduce((sum, item) => sum + item.size, 0);
  return { count: items.length, size };
}
