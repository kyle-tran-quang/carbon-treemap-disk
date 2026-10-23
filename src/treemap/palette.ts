/**
 * Treemap colors. Read from the live Carbon CSS custom properties, so the
 * canvas follows whichever theme class is active and never hard-codes a
 * palette of its own. The fallbacks are the Carbon `white` theme values and
 * only apply if a token is missing.
 */
import type { Tile } from './layout';

export interface TilePaint {
  fill: string;
  text: string;
}

export interface Palette {
  dirEven: TilePaint;
  dirOdd: TilePaint;
  file: TilePaint;
  other: TilePaint;
  inaccessible: TilePaint;
  /** Drawn between tiles. */
  gap: string;
  /** Selection outline, and the thin line inside it. */
  focus: string;
  focusInset: string;
  /** Hover outline. */
  outline: string;
}

/** Carbon tag color names used for each tile category. */
export const TAG_COLORS = {
  dirEven: 'blue',
  dirOdd: 'purple',
  file: 'teal',
  other: 'cool-gray',
  inaccessible: 'red',
} as const;

type PaintKey = keyof typeof TAG_COLORS;

const FALLBACK: Record<PaintKey, TilePaint> = {
  dirEven: { fill: '#d0e2ff', text: '#0043ce' },
  dirOdd: { fill: '#e8daff', text: '#6929c4' },
  file: { fill: '#9ef0f0', text: '#005d5d' },
  other: { fill: '#dde1e6', text: '#4d5358' },
  inaccessible: { fill: '#ffd7d9', text: '#a2191f' },
};

export interface StyleSource {
  getPropertyValue(name: string): string;
}

function token(styles: StyleSource, name: string, fallback: string): string {
  const value = styles.getPropertyValue(`--cds-${name}`).trim();
  return value || fallback;
}

export function readPalette(styles: StyleSource): Palette {
  const paint = (key: PaintKey): TilePaint => ({
    fill: token(styles, `tag-background-${TAG_COLORS[key]}`, FALLBACK[key].fill),
    text: token(styles, `tag-color-${TAG_COLORS[key]}`, FALLBACK[key].text),
  });
  return {
    dirEven: paint('dirEven'),
    dirOdd: paint('dirOdd'),
    file: paint('file'),
    other: paint('other'),
    inaccessible: paint('inaccessible'),
    gap: token(styles, 'background', '#ffffff'),
    focus: token(styles, 'focus', '#0f62fe'),
    focusInset: token(styles, 'focus-inset', '#ffffff'),
    outline: token(styles, 'text-primary', '#161616'),
  };
}

export function paintFor(
  palette: Palette,
  tile: Pick<Tile, 'kind' | 'depth'>,
): TilePaint {
  switch (tile.kind) {
    case 'dir':
      return tile.depth % 2 === 0 ? palette.dirEven : palette.dirOdd;
    case 'file':
      return palette.file;
    case 'inaccessible':
      return palette.inaccessible;
    default:
      return palette.other;
  }
}

function channel(value: number): number {
  const s = value / 255;
  return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
}

function luminance(hex: string): number {
  const digits = hex.replace('#', '');
  const full =
    digits.length === 3
      ? digits
          .split('')
          .map((c) => c + c)
          .join('')
      : digits;
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(full.slice(i, i + 2), 16));
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG 2.x contrast ratio between two `#rgb` or `#rrggbb` colors. */
export function contrastRatio(a: string, b: string): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (light + 0.05) / (dark + 0.05);
}
