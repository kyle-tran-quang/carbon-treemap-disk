import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { formatBytes, formatPercent } from '../format';
import type { ViewNode } from '../ipc';
import { OTHER_LABEL, hitTest, layoutTreemap, tileLabel, type Tile } from './layout';
import { readPalette } from './palette';
import { drawTreemap } from './render';
import { toSelectedItem, type SelectedItem } from './selection';

interface TreemapCanvasProps {
  view: ViewNode;
  /** Absolute path of `view`. */
  path: string;
  /** Changes when the Carbon theme changes, so colors are re-read. */
  theme: string;
  selectedIds: ReadonlySet<number>;
  /** `additive` is true for shift, command or control clicks. */
  onSelect: (item: SelectedItem, additive: boolean) => void;
  onOpen: (id: number) => void;
}

interface Hover {
  tile: Tile;
  x: number;
  y: number;
}

interface Size {
  width: number;
  height: number;
}

const TOOLTIP_OFFSET = 14;
const TOOLTIP_FLIP_MARGIN = 260;

export function TreemapCanvas({
  view,
  path,
  theme,
  selectedIds,
  onSelect,
  onOpen,
}: TreemapCanvasProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState<Size>({ width: 0, height: 0 });
  const [hover, setHover] = useState<Hover | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const observer = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      setSize((prev) =>
        prev.width === width && prev.height === height ? prev : { width, height },
      );
    });
    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  const tiles = useMemo(
    () => layoutTreemap(view, path, size.width, size.height),
    [view, path, size.width, size.height],
  );

  // The tiles change when the view does; a stale hover would point at a gone tile.
  useEffect(() => setHover(null), [tiles]);

  const hoveredKey = hover?.tile.key ?? null;

  // Passive effect: runs after the theme class has been applied to <html>.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || size.width <= 0 || size.height <= 0) return;
    const ratio = window.devicePixelRatio || 1;
    canvas.width = Math.round(size.width * ratio);
    canvas.height = Math.round(size.height * ratio);
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    drawTreemap(ctx, tiles, size.width, size.height, {
      palette: readPalette(getComputedStyle(document.documentElement)),
      selectedIds,
      hoveredKey,
    });
  }, [tiles, size, theme, selectedIds, hoveredKey]);

  const pointAt = useCallback(
    (clientX: number, clientY: number) => {
      const bounds = canvasRef.current?.getBoundingClientRect();
      if (!bounds) return null;
      const x = clientX - bounds.left;
      const y = clientY - bounds.top;
      return { x, y, tile: hitTest(tiles, x, y) };
    },
    [tiles],
  );

  const total = view.size;
  const flip = hover !== null && hover.x > size.width - TOOLTIP_FLIP_MARGIN;

  return (
    <div ref={containerRef} className="treemap">
      <canvas
        ref={canvasRef}
        className="treemap__canvas"
        style={{ width: size.width, height: size.height }}
        role="img"
        aria-label={`Treemap of ${path}, ${formatBytes(total)}. The table below lists the same items for keyboard and screen reader use.`}
        onPointerMove={(event) => {
          const point = pointAt(event.clientX, event.clientY);
          setHover(point?.tile ? { tile: point.tile, x: point.x, y: point.y } : null);
        }}
        onPointerLeave={() => setHover(null)}
        onClick={(event) => {
          const tile = pointAt(event.clientX, event.clientY)?.tile;
          const item = tile ? toSelectedItem(tile) : null;
          if (item) onSelect(item, event.shiftKey || event.metaKey || event.ctrlKey);
        }}
        onDoubleClick={(event) => {
          const tile = pointAt(event.clientX, event.clientY)?.tile;
          if (tile?.kind === 'dir' && tile.id !== null) onOpen(tile.id);
        }}
      />
      {hover && (
        <div
          className="treemap__tooltip"
          aria-hidden="true"
          style={{
            left: flip ? hover.x - TOOLTIP_OFFSET : hover.x + TOOLTIP_OFFSET,
            top: hover.y + TOOLTIP_OFFSET,
            transform: flip ? 'translateX(-100%)' : undefined,
          }}
        >
          <strong className="treemap__tooltip-name">{tileLabel(hover.tile)}</strong>
          {hover.tile.kind !== 'other' && (
            <span className="treemap__tooltip-path">{hover.tile.path}</span>
          )}
          <span>
            {formatBytes(hover.tile.size)} ({formatPercent(hover.tile.size, total)} of
            this folder)
          </span>
          {hover.tile.kind === 'dir' && (
            <span>{hover.tile.files.toLocaleString()} files</span>
          )}
          {hover.tile.kind === 'other' && (
            <span>
              {hover.tile.files.toLocaleString()} files in {OTHER_LABEL.toLowerCase()}
            </span>
          )}
          {hover.tile.kind === 'inaccessible' && <span>Not accessible</span>}
          {hover.tile.kind === 'dir' && (
            <span className="treemap__tooltip-hint">Double-click to open</span>
          )}
        </div>
      )}
    </div>
  );
}
