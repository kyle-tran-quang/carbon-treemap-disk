import { useCallback, useEffect, useRef, useState } from 'react';
import {
  cancelDiscovery,
  discoverCleanup,
  executeCleanupCandidates,
  onCleanupProgress,
  planCleanupCandidates,
  toCleanupError,
  type CandidateCleanupResult,
  type CleanupError,
  type CleanupPlan,
  type DiscoveryProgress,
  type DiscoveryResult,
} from '../ipc';

export type DiscoveryState =
  | { status: 'idle' }
  | { status: 'searching'; progress: DiscoveryProgress | null }
  | { status: 'ready'; result: DiscoveryResult }
  | { status: 'failed'; error: CleanupError };

export type MoveState =
  | { step: 'closed' }
  | { step: 'planning' }
  | { step: 'preview'; plan: CleanupPlan }
  | { step: 'moving'; plan: CleanupPlan }
  | { step: 'done'; plan: CleanupPlan; result: CandidateCleanupResult }
  | { step: 'error'; error: CleanupError; plan?: CleanupPlan };

/**
 * Safe cache cleanup. `search` only reads. `begin` makes a dry-run plan from
 * the selected candidate ids and shows it; `confirm` executes exactly that
 * plan. Only ids are sent to Rust, never paths, and nothing moves before
 * `confirm`.
 */
export function useCleanupScan() {
  const [discovery, setDiscovery] = useState<DiscoveryState>({ status: 'idle' });
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  const [move, setMove] = useState<MoveState>({ step: 'closed' });
  const searching = useRef(false);
  const busy = useRef(false);

  useEffect(() => {
    const pending = onCleanupProgress((progress) => {
      setDiscovery((prev) =>
        prev.status === 'searching' ? { ...prev, progress } : prev,
      );
    });
    return () => {
      void pending.then((unlisten) => unlisten());
    };
  }, []);

  const search = useCallback(async () => {
    if (searching.current || busy.current) return;
    searching.current = true;
    // Ids belong to one discovery, so a new search starts with nothing selected.
    setSelected(new Set());
    setDiscovery({ status: 'searching', progress: null });
    try {
      setDiscovery({ status: 'ready', result: await discoverCleanup() });
    } catch (raw) {
      setDiscovery({ status: 'failed', error: toCleanupError(raw) });
    } finally {
      searching.current = false;
    }
  }, []);

  const cancelSearch = useCallback(() => {
    void cancelDiscovery();
  }, []);

  const toggle = useCallback((id: number, checked: boolean) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (checked) next.add(id);
      else next.delete(id);
      return next;
    });
  }, []);

  const selectAll = useCallback(() => {
    if (discovery.status !== 'ready') return;
    setSelected(new Set(discovery.result.candidates.map((c) => c.id)));
  }, [discovery]);

  const clearSelection = useCallback(() => setSelected(new Set()), []);

  const begin = useCallback(async () => {
    if (busy.current || searching.current || selected.size === 0) return;
    busy.current = true;
    setMove({ step: 'planning' });
    try {
      setMove({ step: 'preview', plan: await planCleanupCandidates([...selected]) });
    } catch (raw) {
      setMove({ step: 'error', error: toCleanupError(raw) });
    } finally {
      busy.current = false;
    }
  }, [selected]);

  const confirm = useCallback(
    async (acknowledged: boolean) => {
      if (busy.current || move.step !== 'preview') return;
      const { plan } = move;
      busy.current = true;
      setMove({ step: 'moving', plan });
      try {
        const result = await executeCleanupCandidates(plan.planId, acknowledged);
        const gone = new Set(
          result.results.filter((r) => r.outcome !== 'failed').map((r) => r.node),
        );
        // Rust drops the same items from its list. Failed ones stay offered.
        setDiscovery((prev) =>
          prev.status === 'ready'
            ? { status: 'ready', result: withoutItems(prev.result, gone) }
            : prev,
        );
        setSelected((prev) => new Set([...prev].filter((id) => !gone.has(id))));
        setMove({ step: 'done', plan, result });
      } catch (raw) {
        setMove({ step: 'error', error: toCleanupError(raw), plan });
      } finally {
        busy.current = false;
      }
    },
    [move],
  );

  const close = useCallback(() => {
    if (busy.current) return;
    setMove({ step: 'closed' });
  }, []);

  return {
    discovery,
    selected,
    move,
    search,
    cancelSearch,
    toggle,
    selectAll,
    clearSelection,
    begin,
    confirm,
    close,
  };
}

function withoutItems(
  result: DiscoveryResult,
  gone: ReadonlySet<number>,
): DiscoveryResult {
  const candidates = result.candidates.filter((c) => !gone.has(c.id));
  return {
    ...result,
    candidates,
    totalBytes: candidates.reduce((sum, c) => sum + c.size, 0),
  };
}

export type CleanupScan = ReturnType<typeof useCleanupScan>;
