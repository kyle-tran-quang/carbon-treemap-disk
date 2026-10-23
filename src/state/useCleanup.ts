import { useCallback, useRef, useState } from 'react';
import {
  executeCleanup,
  planCleanup,
  toCleanupError,
  type CleanupError,
  type CleanupPlan,
  type CleanupResult,
} from '../ipc';

export type CleanupState =
  | { step: 'closed' }
  | { step: 'planning' }
  | { step: 'preview'; plan: CleanupPlan }
  | { step: 'moving'; plan: CleanupPlan }
  | { step: 'done'; plan: CleanupPlan; result: CleanupResult }
  | { step: 'error'; error: CleanupError; plan?: CleanupPlan };

interface CleanupCallbacks {
  /** Called once, after items were moved and the tree changed. */
  onMoved: (result: CleanupResult) => void;
}

/**
 * Two-step cleanup: `begin` makes a dry-run plan and shows it; `confirm`
 * executes exactly that plan. Nothing is moved before `confirm`.
 */
export function useCleanup({ onMoved }: CleanupCallbacks) {
  const [state, setState] = useState<CleanupState>({ step: 'closed' });
  const busy = useRef(false);

  const begin = useCallback(async (nodes: number[]) => {
    if (busy.current || nodes.length === 0) return;
    busy.current = true;
    setState({ step: 'planning' });
    try {
      setState({ step: 'preview', plan: await planCleanup(nodes) });
    } catch (raw) {
      setState({ step: 'error', error: toCleanupError(raw) });
    } finally {
      busy.current = false;
    }
  }, []);

  const confirm = useCallback(
    async (acknowledged: boolean) => {
      if (busy.current || state.step !== 'preview') return;
      const { plan } = state;
      busy.current = true;
      setState({ step: 'moving', plan });
      try {
        const result = await executeCleanup(plan.planId, acknowledged);
        setState({ step: 'done', plan, result });
        onMoved(result);
      } catch (raw) {
        setState({ step: 'error', error: toCleanupError(raw), plan });
      } finally {
        busy.current = false;
      }
    },
    [state, onMoved],
  );

  const close = useCallback(() => {
    if (busy.current) return;
    setState({ step: 'closed' });
  }, []);

  return { state, begin, confirm, close };
}
