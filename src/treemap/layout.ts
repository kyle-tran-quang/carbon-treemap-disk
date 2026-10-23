/**
 * Treemap layout. Pure functions, no DOM, so it can be unit-tested.
 *
 * The input is the bounded `ViewNode` tree from `get_view`. The output is a
 * flat list of tiles in paint order (parents before children), which doubles
 * as the hit-test index.
 *
 * Draw cost depends on screen size, not disk size: within each directory,
 * children whose tile would be smaller than `minArea` pixels are folded into
 * one "Smaller items" tile, together with the backend's own `other` node.
 * Sizes are conserved, so every directory's tiles still sum to its size.
 */
import type { ViewKind, ViewNode } from '../ipc';
import { joinPath } from './paths';

export const OTHER_LABEL = 'Smaller items';

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface LayoutOptions {
  /** Space between a directory's edge and its children. */
  pad: number;
  /** Height of the label strip above a directory's children. */
  headerHeight: number;
  headerMinWidth: number;
  headerMinHeight: number;
  /** A directory is only subdivided if its inner area is at least this wide and tall. */
  minInner: number;
  /** Tiles below this many square pixels are folded into "Smaller items". */
  minArea: number;
}

export const DEFAULT_LAYOUT_OPTIONS: LayoutOptions = {
  pad: 3,
  headerHeight: 18,
  headerMinWidth: 72,
  headerMinHeight: 48,
  minInner: 8,
  minArea: 36,
};

export interface Tile {
  /** Unique within one layout. */
  key: string;
  /** Raw tree id. `null` for "Smaller items". */
  id: number | null;
  name: string;
  path: string;
  kind: ViewKind;
  size: number;
  files: number;
  /** 0 for direct children of the view root. */
  depth: number;
  rect: Rect;
  /** Children are drawn inside this tile. */
  nested: boolean;
  /** Height of the label strip inside `rect`, or 0 when there is none. */
  headerHeight: number;
  /** The directory has content that this view did not expand. */
  expandable: boolean;
  /** For "Smaller items": how many nodes were folded in for being too small to draw. */
  hidden: number;
}

export function tileLabel(tile: Pick<Tile, 'kind' | 'name'>): string {
  return tile.kind === 'other' ? OTHER_LABEL : tile.name;
}

function worstRatio(
  maxArea: number,
  minArea: number,
  rowArea: number,
  side: number,
): number {
  if (minArea <= 0 || rowArea <= 0) return Infinity;
  const sideSquared = side * side;
  return Math.max(
    (sideSquared * maxArea) / (rowArea * rowArea),
    (rowArea * rowArea) / (sideSquared * minArea),
  );
}

/**
 * Squarified treemap (Bruls, Huizing, van Wijk). `sizes` must be positive and
 * sorted largest first. Returns one rectangle per size; together they fill
 * `rect` exactly and each has area proportional to its size.
 */
export function squarify(sizes: readonly number[], rect: Rect): Rect[] {
  const count = sizes.length;
  const out: Rect[] = new Array<Rect>(count);
  let total = sizes.reduce((sum, size) => sum + size, 0);
  let { x, y, w, h } = rect;
  let index = 0;

  while (index < count) {
    if (total <= 0 || w <= 0 || h <= 0) {
      for (; index < count; index += 1) out[index] = { x, y, w: 0, h: 0 };
      break;
    }
    const scale = (w * h) / total;
    const side = Math.min(w, h);
    const largest = sizes[index] * scale;

    let end = index + 1;
    let sum = sizes[index];
    let worst = worstRatio(largest, largest, largest, side);
    while (end < count) {
      const nextSum = sum + sizes[end];
      const nextWorst = worstRatio(largest, sizes[end] * scale, nextSum * scale, side);
      if (nextWorst > worst) break;
      sum = nextSum;
      worst = nextWorst;
      end += 1;
    }

    const rowArea = sum * scale;
    if (w >= h) {
      const columnWidth = rowArea / h;
      let cursor = y;
      for (let i = index; i < end; i += 1) {
        const height = (sizes[i] * scale) / columnWidth;
        out[i] = { x, y: cursor, w: columnWidth, h: height };
        cursor += height;
      }
      x += columnWidth;
      w -= columnWidth;
    } else {
      const rowHeight = rowArea / w;
      let cursor = x;
      for (let i = index; i < end; i += 1) {
        const width = (sizes[i] * scale) / rowHeight;
        out[i] = { x: cursor, y, w: width, h: rowHeight };
        cursor += width;
      }
      y += rowHeight;
      h -= rowHeight;
    }
    total -= sum;
    index = end;
  }
  return out;
}

interface Entry {
  /** `null` for "Smaller items". */
  node: ViewNode | null;
  size: number;
  files: number;
  hidden: number;
}

/** Drops empty nodes and folds tiles that would be too small into one entry. */
function fitEntries(children: readonly ViewNode[], rect: Rect, minArea: number): Entry[] {
  const total = children.reduce((sum, child) => sum + child.size, 0);
  if (total <= 0) return [];
  const areaPerByte = (rect.w * rect.h) / total;

  const entries: Entry[] = [];
  const other: Entry = { node: null, size: 0, files: 0, hidden: 0 };
  for (const child of children) {
    if (child.size <= 0) continue;
    if (child.kind === 'other') {
      other.size += child.size;
      other.files += child.files;
    } else if (child.size * areaPerByte < minArea) {
      other.size += child.size;
      other.files += child.files;
      other.hidden += 1;
    } else {
      entries.push({ node: child, size: child.size, files: child.files, hidden: 0 });
    }
  }
  if (other.size > 0) entries.push(other);
  return entries.sort((a, b) => b.size - a.size);
}

function innerRect(
  rect: Rect,
  options: LayoutOptions,
): { rect: Rect; header: number } | null {
  const hasHeader = rect.w >= options.headerMinWidth && rect.h >= options.headerMinHeight;
  const header = hasHeader ? options.headerHeight : 0;
  const top = hasHeader ? header : options.pad;
  const inner = {
    x: rect.x + options.pad,
    y: rect.y + top,
    w: rect.w - 2 * options.pad,
    h: rect.h - top - options.pad,
  };
  if (inner.w < options.minInner || inner.h < options.minInner) return null;
  return { rect: inner, header };
}

function place(
  tiles: Tile[],
  options: LayoutOptions,
  children: readonly ViewNode[],
  parent: { path: string; key: string; depth: number },
  rect: Rect,
): void {
  const entries = fitEntries(children, rect, options.minArea);
  const rects = squarify(
    entries.map((entry) => entry.size),
    rect,
  );

  entries.forEach((entry, index) => {
    const { node } = entry;
    const key = node ? `n${node.id ?? node.name}` : `${parent.key}/other`;
    const tile: Tile = {
      key,
      id: node?.id ?? null,
      name: node?.name ?? '',
      path: node ? joinPath(parent.path, node.name) : parent.path,
      kind: node?.kind ?? 'other',
      size: entry.size,
      files: entry.files,
      depth: parent.depth,
      rect: rects[index],
      nested: false,
      headerHeight: 0,
      expandable: node?.expandable ?? false,
      hidden: entry.hidden,
    };
    tiles.push(tile);

    if (node?.children?.length) {
      const inner = innerRect(tile.rect, options);
      if (inner) {
        tile.nested = true;
        tile.headerHeight = inner.header;
        place(
          tiles,
          options,
          node.children,
          { path: tile.path, key, depth: parent.depth + 1 },
          inner.rect,
        );
      }
    }
  });
}

/**
 * Lays out the children of `root` inside a `width` x `height` box. `rootPath`
 * is the absolute path of `root`; tile paths are derived from it.
 */
export function layoutTreemap(
  root: ViewNode,
  rootPath: string,
  width: number,
  height: number,
  options: LayoutOptions = DEFAULT_LAYOUT_OPTIONS,
): Tile[] {
  const tiles: Tile[] = [];
  if (!(width > 0 && height > 0) || root.size <= 0) return tiles;
  place(
    tiles,
    options,
    root.children ?? [],
    { path: rootPath, key: 'root', depth: 0 },
    { x: 0, y: 0, w: width, h: height },
  );
  return tiles;
}

/** Deepest tile under the point. Children come after parents in paint order. */
export function hitTest(tiles: readonly Tile[], x: number, y: number): Tile | null {
  for (let i = tiles.length - 1; i >= 0; i -= 1) {
    const { rect } = tiles[i];
    if (x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h) {
      return tiles[i];
    }
  }
  return null;
}
