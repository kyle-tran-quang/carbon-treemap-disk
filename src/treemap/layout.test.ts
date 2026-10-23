import { describe, expect, it } from 'vitest';
import type { ViewNode } from '../ipc';
import {
  DEFAULT_LAYOUT_OPTIONS,
  OTHER_LABEL,
  hitTest,
  layoutTreemap,
  squarify,
  tileLabel,
  type Rect,
  type Tile,
} from './layout';

const area = (r: Rect) => r.w * r.h;

function node(
  id: number,
  name: string,
  size: number,
  extra: Partial<ViewNode> = {},
): ViewNode {
  return { id, name, kind: 'file', size, files: 1, expandable: false, ...extra };
}

function dir(id: number, name: string, children: ViewNode[]): ViewNode {
  return node(
    id,
    name,
    children.reduce((s, c) => s + c.size, 0),
    {
      kind: 'dir',
      files: children.reduce((s, c) => s + c.files, 0),
      children,
    },
  );
}

function other(size: number, files = 1): ViewNode {
  return { name: '', kind: 'other', size, files, expandable: false };
}

describe('squarify', () => {
  const box: Rect = { x: 10, y: 20, w: 400, h: 300 };

  it('fills the rectangle exactly with areas proportional to sizes', () => {
    const sizes = [60, 25, 10, 3, 2];
    const rects = squarify(sizes, box);
    const total = rects.reduce((s, r) => s + area(r), 0);
    expect(total).toBeCloseTo(area(box), 6);
    rects.forEach((r, i) => {
      expect(area(r) / area(box)).toBeCloseTo(sizes[i] / 100, 9);
    });
  });

  it('keeps every rectangle inside the box and never overlapping', () => {
    const rects = squarify([50, 30, 20, 20, 10, 5, 5, 1], box);
    for (const r of rects) {
      expect(r.x).toBeGreaterThanOrEqual(box.x - 1e-9);
      expect(r.y).toBeGreaterThanOrEqual(box.y - 1e-9);
      expect(r.x + r.w).toBeLessThanOrEqual(box.x + box.w + 1e-9);
      expect(r.y + r.h).toBeLessThanOrEqual(box.y + box.h + 1e-9);
    }
    for (let i = 0; i < rects.length; i += 1) {
      for (let j = i + 1; j < rects.length; j += 1) {
        const a = rects[i];
        const b = rects[j];
        const overlapW = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
        const overlapH = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
        expect(overlapW > 1e-6 && overlapH > 1e-6).toBe(false);
      }
    }
  });

  it('returns one rectangle for a single size and none for no sizes', () => {
    expect(squarify([5], box)).toEqual([box]);
    expect(squarify([], box)).toEqual([]);
  });

  it('gives equal sizes near-square tiles', () => {
    const rects = squarify(Array<number>(4).fill(1), { x: 0, y: 0, w: 100, h: 100 });
    for (const r of rects) expect(r.w / r.h).toBeCloseTo(1, 6);
  });

  it('returns empty rectangles for a degenerate box', () => {
    const rects = squarify([3, 2], { x: 0, y: 0, w: 0, h: 50 });
    expect(rects.map(area)).toEqual([0, 0]);
  });
});

describe('layoutTreemap', () => {
  const root = dir(1, 'root', [
    dir(2, 'big', [node(3, 'a', 600), node(4, 'b', 300)]),
    node(5, 'c', 80),
    other(20, 4),
  ]);

  it('returns nothing for an empty view or box', () => {
    expect(layoutTreemap(dir(1, 'r', []), '/r', 100, 100)).toEqual([]);
    expect(layoutTreemap(root, '/root', 0, 100)).toEqual([]);
  });

  it('paints parents before their children and derives paths', () => {
    const tiles = layoutTreemap(root, '/root', 800, 600);
    const paths = tiles.map((t) => t.path);
    expect(paths.indexOf('/root/big')).toBeLessThan(paths.indexOf('/root/big/a'));
    expect(paths).toContain('/root/big/b');
    expect(tiles.find((t) => t.name === 'a')?.depth).toBe(1);
  });

  it('top-level tile sizes sum to the root size, with the backend other node kept', () => {
    const tiles = layoutTreemap(root, '/root', 800, 600).filter((t) => t.depth === 0);
    expect(tiles.reduce((s, t) => s + t.size, 0)).toBe(1000);
    const bucket = tiles.find((t) => t.kind === 'other');
    expect(bucket?.size).toBe(20);
    expect(bucket?.id).toBeNull();
    expect(tileLabel(bucket as Tile)).toBe(OTHER_LABEL);
  });

  it('folds tiles that are too small into one Smaller items tile and conserves size', () => {
    const many = dir(1, 'root', [
      node(2, 'huge', 1_000_000),
      ...Array.from({ length: 50 }, (_, i) => node(10 + i, `tiny${i}`, 1)),
      other(5, 7),
    ]);
    const tiles = layoutTreemap(many, '/root', 400, 300);
    expect(tiles.some((t) => t.name.startsWith('tiny'))).toBe(false);
    const bucket = tiles.find((t) => t.kind === 'other');
    expect(bucket?.size).toBe(55);
    expect(bucket?.files).toBe(50 + 7);
    expect(bucket?.hidden).toBe(50);
    expect(tiles.reduce((s, t) => s + t.size, 0)).toBe(1_000_055);
  });

  it('keeps every tile at least minArea pixels, except the Smaller items tile', () => {
    const wide = dir(
      1,
      'root',
      Array.from({ length: 300 }, (_, i) => node(2 + i, `f${i}`, 1 + (i % 17) * 100)),
    );
    for (const tile of layoutTreemap(wide, '/root', 640, 480)) {
      if (tile.kind !== 'other') {
        expect(area(tile.rect)).toBeGreaterThanOrEqual(
          DEFAULT_LAYOUT_OPTIONS.minArea - 1,
        );
      }
    }
  });

  it('caps the tile count by screen size, not by node count', () => {
    const wide = dir(
      1,
      'root',
      Array.from({ length: 5000 }, (_, i) => node(2 + i, `f${i}`, 1000 + i)),
    );
    const tiles = layoutTreemap(wide, '/root', 640, 480);
    const maxTiles = (640 * 480) / DEFAULT_LAYOUT_OPTIONS.minArea + 1;
    expect(tiles.length).toBeLessThanOrEqual(maxTiles);
  });

  it('nests children inside their directory, leaving room for the header', () => {
    const tiles = layoutTreemap(root, '/root', 800, 600);
    const parent = tiles.find((t) => t.name === 'big') as Tile;
    expect(parent.nested).toBe(true);
    expect(parent.headerHeight).toBe(DEFAULT_LAYOUT_OPTIONS.headerHeight);
    for (const child of tiles.filter((t) => t.path.startsWith('/root/big/'))) {
      expect(child.rect.y).toBeGreaterThanOrEqual(parent.rect.y + parent.headerHeight);
      expect(child.rect.x).toBeGreaterThanOrEqual(parent.rect.x);
      expect(child.rect.x + child.rect.w).toBeLessThanOrEqual(
        parent.rect.x + parent.rect.w,
      );
      expect(child.rect.y + child.rect.h).toBeLessThanOrEqual(
        parent.rect.y + parent.rect.h,
      );
    }
  });

  it('draws a directory as a plain block when it is too small to subdivide', () => {
    const tiles = layoutTreemap(root, '/root', 14, 14);
    const parent = tiles.find((t) => t.name === 'big');
    expect(parent?.nested ?? false).toBe(false);
    expect(tiles.some((t) => t.name === 'a')).toBe(false);
  });

  it('does not nest an unexpanded directory and keeps its true size', () => {
    const view = dir(1, 'root', [
      node(2, 'deep', 900, { kind: 'dir', expandable: true }),
      node(3, 'x', 100),
    ]);
    const deep = layoutTreemap(view, '/root', 400, 300).find(
      (t) => t.name === 'deep',
    ) as Tile;
    expect(deep.nested).toBe(false);
    expect(deep.expandable).toBe(true);
    expect(deep.size).toBe(900);
  });

  it('gives every tile a unique key', () => {
    const tiles = layoutTreemap(root, '/root', 800, 600);
    expect(new Set(tiles.map((t) => t.key)).size).toBe(tiles.length);
  });
});

describe('hitTest', () => {
  const root = dir(1, 'root', [
    dir(2, 'big', [node(3, 'a', 600), node(4, 'b', 300)]),
    node(5, 'c', 100),
  ]);
  const tiles = layoutTreemap(root, '/root', 800, 600);

  it('returns the deepest tile under the point', () => {
    const a = tiles.find((t) => t.name === 'a') as Tile;
    const hit = hitTest(tiles, a.rect.x + a.rect.w / 2, a.rect.y + a.rect.h / 2);
    expect(hit?.name).toBe('a');
  });

  it('returns the directory when the point is on its header', () => {
    const big = tiles.find((t) => t.name === 'big') as Tile;
    const hit = hitTest(tiles, big.rect.x + 4, big.rect.y + 4);
    expect(hit?.name).toBe('big');
  });

  it('returns null outside every tile', () => {
    expect(hitTest(tiles, -5, -5)).toBeNull();
    expect(hitTest(tiles, 900, 10)).toBeNull();
  });
});
