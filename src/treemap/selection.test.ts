import { describe, expect, it } from 'vitest';
import { baseName, crumbPath, joinPath } from './paths';
import {
  selectItem,
  setItems,
  summarize,
  toSelectedItem,
  type SelectedItem,
} from './selection';

function item(id: number, path: string, size: number): SelectedItem {
  return { id, path, size, name: path, kind: 'dir', files: 1 };
}

describe('paths', () => {
  it('joins with exactly one slash, including under /', () => {
    expect(joinPath('/Users/me', 'a')).toBe('/Users/me/a');
    expect(joinPath('/', 'Users')).toBe('/Users');
    expect(joinPath('/Users/', 'a')).toBe('/Users/a');
  });

  it('builds the absolute path from breadcrumbs', () => {
    const crumbs = [
      { id: 0, name: '/Users/me' },
      { id: 4, name: 'Documents' },
      { id: 9, name: 'x' },
    ];
    expect(crumbPath(crumbs)).toBe('/Users/me/Documents/x');
    expect(
      crumbPath([
        { id: 0, name: '/' },
        { id: 1, name: 'Users' },
      ]),
    ).toBe('/Users');
    expect(crumbPath([])).toBe('');
  });

  it('returns the last segment for display', () => {
    expect(baseName('/Users/me')).toBe('me');
    expect(baseName('/Users/me/')).toBe('me');
    expect(baseName('/')).toBe('/');
  });
});

describe('selection', () => {
  const a = item(1, '/r/a', 100);
  const b = item(2, '/r/b', 50);

  it('replaces the selection on a plain click', () => {
    expect(selectItem([a], b, false)).toEqual([b]);
    expect(selectItem([a], a, false)).toEqual([a]);
  });

  it('toggles one item on an additive click', () => {
    expect(selectItem([a], b, true)).toEqual([a, b]);
    expect(selectItem([a, b], a, true)).toEqual([b]);
  });

  it('adds or removes a whole subset without duplicates', () => {
    expect(setItems([a], [a, b], true).map((i) => i.id)).toEqual([1, 2]);
    expect(setItems([a, b], [a], false)).toEqual([b]);
  });

  it('counts an item once when an ancestor is also selected', () => {
    const child = item(3, '/r/a/c', 40);
    expect(summarize([a, b, child])).toEqual({ count: 3, size: 150 });
  });

  it('does not treat a sibling that shares a name prefix as a descendant', () => {
    const sibling = item(4, '/r/ab', 7);
    expect(summarize([a, sibling])).toEqual({ count: 2, size: 107 });
  });

  it('summarizes an empty selection', () => {
    expect(summarize([])).toEqual({ count: 0, size: 0 });
  });

  it('only allows real directories and files to be selected', () => {
    const base = { name: 'n', path: '/n', size: 1, files: 1 };
    expect(toSelectedItem({ ...base, id: 1, kind: 'dir' })?.kind).toBe('dir');
    expect(toSelectedItem({ ...base, id: null, kind: 'other' })).toBeNull();
    expect(toSelectedItem({ ...base, id: 2, kind: 'inaccessible' })).toBeNull();
    expect(toSelectedItem({ ...base, id: undefined, kind: 'file' })).toBeNull();
  });
});
