import { Button, InlineNotification, ProgressBar, Tag, Toggle } from '@carbon/react';
import FolderOpen from '@carbon/icons-react/es/FolderOpen';
import { formatBytes } from '../format';
import type { ScanError } from '../ipc';
import type { ScanState } from '../state/useScan';
import { ScanIssueList } from './ScanIssueList';

interface ScanPanelProps {
  state: ScanState;
  recentRoots: readonly string[];
  includeCloud: boolean;
  onIncludeCloudChange: (value: boolean) => void;
  onChoose: () => void;
  onScan: (root: string) => void;
  onCancel: () => void;
}

function errorTitle(error: ScanError): string {
  switch (error.kind) {
    case 'permissionDenied':
      return 'Permission denied';
    case 'notFound':
      return 'Folder not found';
    case 'alreadyRunning':
      return 'A scan is already running';
    default:
      return 'Scan failed';
  }
}

export function ScanPanel({
  state,
  recentRoots,
  includeCloud,
  onIncludeCloudChange,
  onChoose,
  onScan,
  onCancel,
}: ScanPanelProps) {
  const scanning = state.status === 'scanning';

  return (
    <section className="app__scan" aria-label="Scan">
      <div className="app__scan-actions">
        <Button
          kind="primary"
          renderIcon={FolderOpen}
          disabled={scanning}
          onClick={onChoose}
        >
          Choose folder
        </Button>
        {scanning && (
          <Button kind="secondary" onClick={onCancel}>
            Cancel scan
          </Button>
        )}
      </div>

      <Toggle
        id="include-cloud-folders"
        size="sm"
        labelText="Cloud-synced folders"
        labelA="Skipped"
        labelB="Included (can be slow)"
        toggled={includeCloud}
        disabled={scanning}
        onToggle={onIncludeCloudChange}
      />

      {recentRoots.length > 0 && !scanning && (
        <div className="app__recent">
          <p className="app__recent-label">Recent folders</p>
          <ul className="app__recent-list">
            {recentRoots.map((root) => (
              <li key={root}>
                <Tag
                  as="button"
                  type="gray"
                  title={root}
                  aria-label={`Scan ${root}`}
                  onClick={() => onScan(root)}
                >
                  {root}
                </Tag>
              </li>
            ))}
          </ul>
        </div>
      )}

      {state.status === 'scanning' && (
        <ProgressBar
          label={`Scanning ${state.root}`}
          helperText={
            state.progress
              ? `${state.progress.itemsScanned.toLocaleString()} items, ${formatBytes(state.progress.bytes)}`
              : 'Starting'
          }
        />
      )}

      {state.status === 'done' && (
        <>
          <InlineNotification
            kind="success"
            lowContrast
            hideCloseButton
            title="Scan complete"
            subtitle={`${state.summary.fileCount.toLocaleString()} files, ${formatBytes(state.summary.totalBytes)} in ${state.root}`}
          />
          {state.summary.skippedCloudFolders.length > 0 && (
            <InlineNotification
              kind="info"
              lowContrast
              hideCloseButton
              title="Cloud-synced folders were skipped"
              subtitle={`${state.summary.skippedCloudFolders.join(', ')}. Listing them can take minutes and they are not counted in these totals. Turn on "Cloud-synced folders" and scan again to include them.`}
            />
          )}
          <ScanIssueList issues={state.summary.issues} />
        </>
      )}

      {state.status === 'failed' && (
        <InlineNotification
          kind={state.error.kind === 'cancelled' ? 'info' : 'error'}
          lowContrast
          hideCloseButton
          title={
            state.error.kind === 'cancelled' ? 'Scan cancelled' : errorTitle(state.error)
          }
          subtitle={state.error.kind === 'cancelled' ? state.root : state.error.message}
        />
      )}
    </section>
  );
}
