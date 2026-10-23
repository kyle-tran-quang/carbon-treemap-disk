import { useCallback, useState } from 'react';
import { Content, GlobalTheme, HeaderContainer } from '@carbon/react';
import { AppHeader } from './components/AppHeader';
import { AppSideNav } from './components/AppSideNav';
import { CleanupModal } from './components/CleanupModal';
import { FullDiskAccessNotice } from './components/FullDiskAccessNotice';
import { ScanPanel } from './components/ScanPanel';
import { TreemapExplorer } from './components/TreemapExplorer';
import { pickFolder, type CleanupResult } from './ipc';
import { addRecentRoot, loadRecentRoots, saveRecentRoots } from './state/recentRoots';
import { useCleanup } from './state/useCleanup';
import { useExplorer } from './state/useExplorer';
import { useScan } from './state/useScan';
import { ThemeProvider, useTheme } from './state/ThemeContext';
import './styles/app.scss';

function AppShell() {
  const { theme } = useTheme();
  const [recentRoots, setRecentRoots] = useState(loadRecentRoots);

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

  return (
    <GlobalTheme theme={theme}>
      <div className="app">
        <HeaderContainer
          render={({ isSideNavExpanded, onClickSideNavExpand }) => (
            <>
              <AppHeader
                isSideNavExpanded={isSideNavExpanded}
                onClickSideNavExpand={onClickSideNavExpand}
              />
              <AppSideNav isSideNavExpanded={isSideNavExpanded} />
            </>
          )}
        />
        <Content id="main-content" className="app__content">
          <h1>Disk Treemap</h1>
          <p>Select a folder to scan and visualize disk usage.</p>
          <FullDiskAccessNotice />
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
            cleanupDisabled={scanning}
            onCleanup={() =>
              void cleanup.begin(explorer.selection.map((item) => item.id))
            }
          />
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
