import { useCallback, useEffect, useRef, useState } from 'react';
import { Content, GlobalTheme, HeaderContainer } from '@carbon/react';
import { AppHeader } from './components/AppHeader';
import { AppSideNav } from './components/AppSideNav';
import { CleanupModal } from './components/CleanupModal';
import { SelectionActionBar } from './components/SelectionActionBar';
import { CleanupView } from './components/CleanupView';
import { FullDiskAccessNotice } from './components/FullDiskAccessNotice';
import { ScanPanel } from './components/ScanPanel';
import { TreemapExplorer } from './components/TreemapExplorer';
import { pickFolder, type CleanupResult } from './ipc';
import { addRecentRoot, loadRecentRoots, saveRecentRoots } from './state/recentRoots';
import { useCleanup } from './state/useCleanup';
import { useCleanupScan } from './state/useCleanupScan';
import { useExplorer } from './state/useExplorer';
import { useScan } from './state/useScan';
import { ThemeProvider, useTheme } from './state/ThemeContext';
import type { AppView } from './views';
import './styles/app.scss';

function AppShell() {
  const { theme } = useTheme();
  const [recentRoots, setRecentRoots] = useState(loadRecentRoots);
  const [view, setView] = useState<AppView>('treemap');
  const cleanupScan = useCleanupScan();

  // Switching views unmounts the item that had focus, so move focus to the new
  // page heading. Skipped on first render so startup focus is left alone.
  const shownView = useRef(view);
  useEffect(() => {
    if (shownView.current === view) return;
    shownView.current = view;
    document.getElementById('main-content')?.querySelector('h1')?.focus();
  }, [view]);

  const rememberRoot = useCallback((root: string) => {
    setRecentRoots((prev) => {
      const next = addRecentRoot(prev, root);
      saveRecentRoots(next);
      return next;
    });
  }, []);

  const { state, start, cancel, includeCloud, setIncludeCloud } = useScan(rememberRoot);
  const explorer = useExplorer(state);
  const { dropFromSelection, refresh } = explorer;

  const onMoved = useCallback(
    (result: CleanupResult) => {
      dropFromSelection(
        result.results.filter((r) => r.outcome !== 'failed').map((r) => r.node),
      );
      if (result.treeUpdated) void refresh();
    },
    [dropFromSelection, refresh],
  );
  const cleanup = useCleanup({ onMoved });
  const scanning = state.status === 'scanning';

  const choose = useCallback(async () => {
    const root = await pickFolder();
    if (root) void start(root);
  }, [start]);

  const cleanupCandidates =
    cleanupScan.discovery.status === 'ready'
      ? cleanupScan.discovery.result.candidates
      : [];

  return (
    <GlobalTheme theme={theme}>
      <div className="app">
        <HeaderContainer
          render={({ isSideNavExpanded, onClickSideNavExpand }) => (
            <>
              <AppHeader
                isSideNavExpanded={isSideNavExpanded}
                onClickSideNavExpand={onClickSideNavExpand}
                view={view}
                onNavigate={setView}
              />
              <AppSideNav
                isSideNavExpanded={isSideNavExpanded}
                view={view}
                onNavigate={setView}
              />
            </>
          )}
        />
        <Content id="main-content" className="app__content">
          <FullDiskAccessNotice />
          {view === 'treemap' ? (
            <>
              <h1 tabIndex={-1}>Disk Treemap</h1>
              <p>Select a folder to scan and visualize disk usage.</p>
              <ScanPanel
                state={state}
                recentRoots={recentRoots}
                includeCloud={includeCloud}
                onIncludeCloudChange={setIncludeCloud}
                onChoose={() => void choose()}
                onScan={(root) => void start(root)}
                onCancel={cancel}
              />
              <TreemapExplorer
                explorer={explorer}
                theme={theme}
              />
            </>
          ) : (
            <CleanupView scan={cleanupScan} />
          )}
          <CleanupModal
            state={cleanup.state}
            onConfirm={(acknowledged) => void cleanup.confirm(acknowledged)}
            onClose={cleanup.close}
            onRescan={() => {
              cleanup.close();
              if (state.status === 'done') void start(state.root);
            }}
          />
        </Content>
        {/* Portalled outside .app so position:fixed isn't clipped by
            overflow:hidden or will-change on any ancestor. */}
        {view === 'treemap' ? (
          <SelectionActionBar
            count={explorer.selectionSummary.count}
            bytes={explorer.selectionSummary.size}
            singlePath={
              explorer.selection.length === 1 ? explorer.selection[0].path : undefined
            }
            singleName={
              explorer.selection.length === 1 ? explorer.selection[0].name : undefined
            }
            primaryLabel="Preview move to Trash"
            primaryDisabled={scanning}
            onPrimary={() =>
              void cleanup.begin(explorer.selection.map((item) => item.id))
            }
          />
        ) : (
          <SelectionActionBar
            count={cleanupScan.selected.size}
            bytes={cleanupCandidates
              .filter((c) => cleanupScan.selected.has(c.id))
              .reduce((sum, c) => sum + c.size, 0)}
            singlePath={
              cleanupScan.selected.size === 1
                ? cleanupCandidates.find((c) => cleanupScan.selected.has(c.id))?.path
                : undefined
            }
            singleName={
              cleanupScan.selected.size === 1
                ? cleanupCandidates.find((c) => cleanupScan.selected.has(c.id))?.name
                : undefined
            }
            primaryLabel="Review and move to Trash"
            primaryDisabled={cleanupScan.selected.size === 0 || cleanupScan.move.step !== 'closed'}
            onPrimary={() => void cleanupScan.begin()}
          />
        )}
      </div>
    </GlobalTheme>
  );
}

export default function App() {
  return (
    <ThemeProvider>
      <AppShell />
    </ThemeProvider>
  );
}
