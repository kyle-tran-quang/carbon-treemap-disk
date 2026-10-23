import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { findNodeByPath, getView, type ViewResponse } from '../ipc';
import { crumbPath } from '../treemap/paths';
import { selectItem, setItems, summarize, type SelectedItem } from '../treemap/selection';
import type { ScanState } from './useScan';

export interface ExplorerView {
  response: ViewResponse;
  /** Absolute path of the folder being shown. */
  path: string;
}

function messageOf(raw: unknown): string {
  if (typeof raw === 'object' && raw !== null) {
    const { message } = raw as { message?: unknown };
    if (typeof message === 'string') return message;
  }
  return String(raw);
}

/**
 * Current folder, selection and navigation over the scanned tree.
 *
 * Raw node ids are only valid for the scan that produced them, so the
 * location is remembered by path and restored through `find_node_by_path`
 * after a rescan. The selection is cleared on a new scan for the same reason.
 */
export function useExplorer(scan: ScanState) {
  const summary = scan.status === 'done' ? scan.summary : null;
  const [current, setCurrent] = useState<ExplorerView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [selection, setSelection] = useState<SelectedItem[]>([]);
  const sequence = useRef(0);
  const lastPath = useRef<string | null>(null);

  const load = useCallback(async (node?: number) => {
    const ticket = ++sequence.current;
    setLoading(true);
    setError(null);
    try {
      const response = await getView({ node });
      if (ticket !== sequence.current) return;
      const path = crumbPath(response.breadcrumbs);
      lastPath.current = path;
      setCurrent({ response, path });
    } catch (raw) {
      if (ticket !== sequence.current) return;
      setError(messageOf(raw));
    } finally {
      if (ticket === sequence.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!summary) return;
    setSelection([]);
    const previous = lastPath.current;
    void (async () => {
      let node: number | undefined;
      if (previous) {
        try {
          node = (await findNodeByPath(previous)) ?? undefined;
        } catch {
          node = undefined;
        }
      }
      await load(node);
    })();
  }, [summary, load]);

  const crumbs = current?.response.breadcrumbs;
  const currentNode = crumbs?.[crumbs.length - 1]?.id;

  /**
   * Reloads the folder on screen after the tree changed in place. If that
   * folder is itself gone, falls back to the scan root.
   */
  const refresh = useCallback(async () => {
    const ticket = ++sequence.current;
    setLoading(true);
    setError(null);
    try {
      let response: ViewResponse;
      try {
        response = await getView({ node: currentNode });
      } catch {
        response = await getView({});
      }
      if (ticket !== sequence.current) return;
      const path = crumbPath(response.breadcrumbs);
      lastPath.current = path;
      setCurrent({ response, path });
    } catch (raw) {
      if (ticket === sequence.current) setError(messageOf(raw));
    } finally {
      if (ticket === sequence.current) setLoading(false);
    }
  }, [currentNode]);

  const dropFromSelection = useCallback((ids: readonly number[]) => {
    const gone = new Set(ids);
    setSelection((prev) => prev.filter((item) => !gone.has(item.id)));
  }, []);

  const select = useCallback((item: SelectedItem, additive: boolean) => {
    setSelection((prev) => selectItem(prev, item, additive));
  }, []);

  const setMany = useCallback((items: readonly SelectedItem[], on: boolean) => {
    setSelection((prev) => setItems(prev, items, on));
  }, []);

  const clearSelection = useCallback(() => setSelection([]), []);

  const selectedIds = useMemo(
    () => new Set(selection.map((item) => item.id)),
    [selection],
  );
  const selectionSummary = useMemo(() => summarize(selection), [selection]);

  return {
    current: summary ? current : null,
    error,
    loading,
    open: load,
    refresh,
    dropFromSelection,
    selection,
    selectedIds,
    selectionSummary,
    select,
    setMany,
    clearSelection,
  };
}
