import { useCallback, useEffect, useRef, useState } from 'react';
import {
  cancelScan,
  onScanProgress,
  startScan,
  type ScanError,
  type ScanProgress,
  type ScanSummary,
} from '../ipc';
import { loadIncludeCloud, saveIncludeCloud } from './scanSettings';

export type ScanState =
  | { status: 'idle' }
  | { status: 'scanning'; root: string; progress: ScanProgress | null }
  | { status: 'done'; root: string; summary: ScanSummary }
  | { status: 'failed'; root: string; error: ScanError };

export function useScan(onStarted?: (root: string) => void) {
  const [state, setState] = useState<ScanState>({ status: 'idle' });
  const [includeCloud, setIncludeCloudState] = useState(loadIncludeCloud);
  const running = useRef(false);
  const includeCloudRef = useRef(includeCloud);

  useEffect(() => {
    const pending = onScanProgress((progress) => {
      setState((prev) => (prev.status === 'scanning' ? { ...prev, progress } : prev));
    });
    return () => {
      void pending.then((unlisten) => unlisten());
    };
  }, []);

  const setIncludeCloud = useCallback((value: boolean) => {
    includeCloudRef.current = value;
    setIncludeCloudState(value);
    saveIncludeCloud(value);
  }, []);

  const start = useCallback(
    async (root: string) => {
      if (running.current) return;
      running.current = true;
      setState({ status: 'scanning', root, progress: null });
      onStarted?.(root);
      try {
        const summary = await startScan(root, includeCloudRef.current);
        setState({ status: 'done', root, summary });
      } catch (raw) {
        setState({ status: 'failed', root, error: raw as ScanError });
      } finally {
        running.current = false;
      }
    },
    [onStarted],
  );

  const cancel = useCallback(() => {
    void cancelScan();
  }, []);

  return { state, start, cancel, includeCloud, setIncludeCloud };
}
