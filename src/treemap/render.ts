import { tileLabel, type Tile } from './layout';
import { paintFor, type Palette } from './palette';

const FONT_FAMILY = "'IBM Plex Sans', 'Helvetica Neue', Arial, sans-serif";
const LABEL_FONT = `600 12px ${FONT_FAMILY}`;
const LABEL_INSET = 6;
/** Narrowest tile that still gets a label. */
const MIN_LABEL_WIDTH = 36;

export interface DrawState {
  palette: Palette;
  selectedIds: ReadonlySet<number>;
  hoveredKey: string | null;
}

/** Shortens `text` with an ellipsis so it fits in `maxWidth`. Empty if nothing fits. */
function fitText(ctx: CanvasRenderingContext2D, text: string, maxWidth: number): string {
  if (ctx.measureText(text).width <= maxWidth) return text;
  let low = 0;
  let high = text.length;
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    if (ctx.measureText(`${text.slice(0, mid)}…`).width <= maxWidth) low = mid;
    else high = mid - 1;
  }
  return low === 0 ? '' : `${text.slice(0, low)}…`;
}

function drawLabel(ctx: CanvasRenderingContext2D, tile: Tile, color: string): void {
  const { rect } = tile;
  const textWidth = rect.w - 2 * LABEL_INSET;
  if (rect.w < MIN_LABEL_WIDTH || textWidth <= 0) return;
  const label = fitText(ctx, tileLabel(tile), textWidth);
  if (!label) return;

  const height = tile.nested ? tile.headerHeight : rect.h;
  if (height < 16) return;

  ctx.fillStyle = color;
  ctx.textBaseline = 'top';
  ctx.fillText(label, rect.x + LABEL_INSET, rect.y + 3);
}

function strokeInside(
  ctx: CanvasRenderingContext2D,
  tile: Tile,
  color: string,
  width: number,
  inset: number,
): void {
  const { rect } = tile;
  ctx.strokeStyle = color;
  ctx.lineWidth = width;
  const half = inset + width / 2;
  ctx.strokeRect(rect.x + half, rect.y + half, rect.w - 2 * half, rect.h - 2 * half);
}

/**
 * Paints the tiles. Coordinates are CSS pixels; the caller scales the context
 * for the device pixel ratio.
 */
export function drawTreemap(
  ctx: CanvasRenderingContext2D,
  tiles: readonly Tile[],
  width: number,
  height: number,
  state: DrawState,
): void {
  const { palette } = state;
  ctx.clearRect(0, 0, width, height);
  ctx.fillStyle = palette.gap;
  ctx.fillRect(0, 0, width, height);
  ctx.font = LABEL_FONT;

  for (const tile of tiles) {
    const { rect } = tile;
    if (rect.w < 1 || rect.h < 1) continue;
    const paint = paintFor(palette, tile);
    // Leave a 1px seam so neighbouring tiles read as separate blocks.
    ctx.fillStyle = paint.fill;
    ctx.fillRect(
      rect.x + 0.5,
      rect.y + 0.5,
      Math.max(0, rect.w - 1),
      Math.max(0, rect.h - 1),
    );
    drawLabel(ctx, tile, paint.text);
  }

  for (const tile of tiles) {
    if (tile.id !== null && state.selectedIds.has(tile.id)) {
      strokeInside(ctx, tile, palette.focus, 3, 0);
      strokeInside(ctx, tile, palette.focusInset, 1, 3);
    }
  }

  const hovered = tiles.find((tile) => tile.key === state.hoveredKey);
  if (hovered) strokeInside(ctx, hovered, palette.outline, 2, 0);
}
