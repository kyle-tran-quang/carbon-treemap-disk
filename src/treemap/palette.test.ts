import { describe, expect, it } from 'vitest';
import { g100, tagTokens, white } from '@carbon/themes';
import {
  TAG_COLORS,
  contrastRatio,
  paintFor,
  readPalette,
  type Palette,
} from './palette';

const camel = (name: string) =>
  name.replace(/-([a-z])/g, (_, c: string) => c.toUpperCase());

/** Same lookup the canvas does, but from Carbon's published theme data. */
function paletteFor(
  theme: 'whiteTheme' | 'g100',
  tokens: Record<string, unknown>,
): Palette {
  const tags = tagTokens as unknown as Record<string, Record<string, string>>;
  return readPalette({
    getPropertyValue(name: string): string {
      const token = camel(name.replace('--cds-', ''));
      const tag = tags[token];
      return tag ? tag[theme] : String(tokens[token] ?? '');
    },
  });
}

const categories = Object.keys(TAG_COLORS) as (keyof typeof TAG_COLORS)[];

describe.each([
  ['white', paletteFor('whiteTheme', white)],
  ['g100', paletteFor('g100', g100)],
])('palette in the %s theme', (_name, palette) => {
  it.each(categories)('%s label text meets WCAG AA (4.5:1) on its fill', (key) => {
    expect(contrastRatio(palette[key].text, palette[key].fill)).toBeGreaterThanOrEqual(
      4.5,
    );
  });

  it.each(categories)('%s tile: selection ring meets 3:1 against its fill', (key) => {
    expect(contrastRatio(palette.focus, palette[key].fill)).toBeGreaterThanOrEqual(3);
  });

  it.each(categories)('%s tile: hover outline meets 3:1 against its fill', (key) => {
    expect(contrastRatio(palette.outline, palette[key].fill)).toBeGreaterThanOrEqual(3);
  });

  it('selection ring meets 3:1 against the gap color and the inset line', () => {
    expect(contrastRatio(palette.focus, palette.gap)).toBeGreaterThanOrEqual(3);
    expect(contrastRatio(palette.focus, palette.focusInset)).toBeGreaterThanOrEqual(3);
  });
});

describe('readPalette', () => {
  it('uses fallbacks when tokens are missing', () => {
    const palette = readPalette({ getPropertyValue: () => '' });
    expect(palette.dirEven.fill).toBe('#d0e2ff');
    expect(palette.gap).toBe('#ffffff');
  });

  it('reads the tag tokens for the configured colors', () => {
    const seen: string[] = [];
    readPalette({
      getPropertyValue(name) {
        seen.push(name);
        return '';
      },
    });
    for (const color of Object.values(TAG_COLORS)) {
      expect(seen).toContain(`--cds-tag-background-${color}`);
      expect(seen).toContain(`--cds-tag-color-${color}`);
    }
  });

  it('alternates directory colors by depth', () => {
    const palette = readPalette({ getPropertyValue: () => '' });
    expect(paintFor(palette, { kind: 'dir', depth: 0 })).toBe(palette.dirEven);
    expect(paintFor(palette, { kind: 'dir', depth: 1 })).toBe(palette.dirOdd);
    expect(paintFor(palette, { kind: 'file', depth: 3 })).toBe(palette.file);
    expect(paintFor(palette, { kind: 'other', depth: 0 })).toBe(palette.other);
  });
});

describe('contrastRatio', () => {
  it('matches known values', () => {
    expect(contrastRatio('#000000', '#ffffff')).toBeCloseTo(21, 5);
    expect(contrastRatio('#fff', '#fff')).toBeCloseTo(1, 5);
  });
});
