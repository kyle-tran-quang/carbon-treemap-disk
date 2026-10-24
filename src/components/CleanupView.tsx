import { useRef } from 'react';
import {
  Button,
  Checkbox,
  InlineNotification,
  ProgressBar,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Tag,
} from '@carbon/react';
import Search from '@carbon/icons-react/es/Search';
import { formatBytes } from '../format';
import type { Candidate, CandidateCategory, CleanupError, DiscoveryResult } from '../ipc';
import type { CleanupScan } from '../state/useCleanupScan';
import { CleanupConfirmModal } from './CleanupConfirmModal';

interface CategoryInfo {
  title: string;
  /** What the user gives up by removing it. Shown above the list. */
  effect: string;
}

const CATEGORIES: readonly [CandidateCategory, CategoryInfo][] = [
  [
    'appCaches',
    {
      title: 'App caches',
      effect:
        'Apps rebuild these files when they need them. The first launch afterwards can be slower.',
    },
  ],
  [
    'appLogs',
    {
      title: 'App logs',
      effect: 'Past log files. Apps write new ones as they run.',
    },
  ],
  [
    'xcodeBuildCache',
    {
      title: 'Xcode build data',
      effect:
        'The next build recompiles and re-indexes, and project folders may download their Swift packages again.',
    },
  ],
  [
    'npmCache',
    {
      title: 'npm cache',
      effect: 'The next install downloads packages again instead of reusing them.',
    },
  ],
];

const pluralItems = (count: number) =>
  `${count.toLocaleString()} ${count === 1 ? 'item' : 'items'}`;

function formatDate(secs: number): string {
  return new Date(secs * 1000).toLocaleDateString();
}

function errorTitle(error: CleanupError): string {
  switch (error.kind) {
    case 'cancelled':
      return 'Search cancelled';
    case 'discoveryRunning':
      return 'A search is already running';
    default:
      return 'Could not search';
  }
}

function CategoryTable({
  info,
  items,
  selected,
  disabled,
  onToggle,
}: {
  info: CategoryInfo;
  items: Candidate[];
  selected: ReadonlySet<number>;
  disabled: boolean;
  onToggle: (id: number, checked: boolean) => void;
}) {
  const bytes = items.reduce((sum, item) => sum + item.size, 0);
  return (
    <section className="cleanup-view__category" aria-label={info.title}>
      <h3 className="cleanup-view__category-title">
        {info.title}{' '}
        <span className="cleanup-view__category-total">
          {pluralItems(items.length)}, {formatBytes(bytes)}
        </span>
      </h3>
      <p className="cleanup__note">{info.effect}</p>
      <Table size="sm" aria-label={`${info.title}, largest first`}>
        <TableHead>
          <TableRow>
            <TableHeader>Select</TableHeader>
            <TableHeader>Path</TableHeader>
            <TableHeader className="cleanup__number">Size</TableHeader>
            <TableHeader className="cleanup__number">Last changed</TableHeader>
          </TableRow>
        </TableHead>
        <TableBody>
          {items.map((item) => (
            <TableRow key={item.id}>
              <TableCell>
                <Checkbox
                  id={`cleanup-candidate-${item.id}`}
                  labelText={`Select ${item.name}`}
                  hideLabel
                  checked={selected.has(item.id)}
                  disabled={disabled}
                  onChange={(_, { checked }) => onToggle(item.id, checked)}
                />
              </TableCell>
              <TableCell className="cleanup__path">
                {item.path}
                {item.kind === 'dir' ? '/' : ''}
                {item.incomplete && (
                  <>
                    {' '}
                    <Tag
                      type="gray"
                      size="sm"
                      title="Some files inside could not be read, so the real size is larger."
                    >
                      Size is a minimum
                    </Tag>
                  </>
                )}
              </TableCell>
              <TableCell className="cleanup__number">{formatBytes(item.size)}</TableCell>
              <TableCell className="cleanup__number">
                {formatDate(item.modifiedSecs)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </section>
  );
}

function SkippedNotes({ result }: { result: DiscoveryResult }) {
  const count = (reason: string) =>
    result.skipped.filter((s) => s.reason === reason).length;
  const recent = count('recent');
  const unreadable = count('unreadable');
  const other = result.skipped.length - recent - unreadable;
  return (
    <>
      {recent > 0 && (
        <p className="cleanup__note">
          {pluralItems(recent)} changed in the last 24 hours, so they are probably in use
          and are not listed.
        </p>
      )}
      {unreadable > 0 && (
        <p className="cleanup__note">
          {pluralItems(unreadable)} could not be read and are not listed. Full Disk Access
          can help.
        </p>
      )}
      {other > 0 && (
        <p className="cleanup__note">
          {pluralItems(other)} were left out because they are links or point outside the
          folder they sit in.
        </p>
      )}
      {result.excluded > 0 && (
        <p className="cleanup__note">
          {pluralItems(result.excluded)} that belong to macOS or to this app are never
          listed.
        </p>
      )}
    </>
  );
}

export function CleanupView({ scan }: { scan: CleanupScan }) {
  const { discovery, selected, move } = scan;
  const headingRef = useRef<HTMLHeadingElement>(null);
  const searching = discovery.status === 'searching';
  const result = discovery.status === 'ready' ? discovery.result : null;
  const candidates = result?.candidates ?? [];

  // After the dialog closes, restore focus to the page heading.
  const closeModal = () => {
    scan.close();
    requestAnimationFrame(() => headingRef.current?.focus());
  };

  return (
    <div className="cleanup-view">
      <h1 ref={headingRef} tabIndex={-1}>
        Clean up caches
      </h1>
      <p>
        Find cache and log folders that apps and developer tools rebuild on their own.
        Nothing is removed until you review a list and confirm. Items go to the Trash, so
        you can put them back.
      </p>
      <p className="cleanup__note">
        For the best result, quit the apps you are about to clean. Anything an app is
        using right now may be rebuilt straight away.
      </p>

      <div className="app__scan-actions">
        <Button
          kind="primary"
          renderIcon={Search}
          disabled={searching || move.step !== 'closed'}
          onClick={() => void scan.search()}
        >
          {result ? 'Search again' : 'Find items to clean up'}
        </Button>
        {searching && (
          <Button kind="secondary" onClick={scan.cancelSearch}>
            Cancel search
          </Button>
        )}
      </div>

      {discovery.status === 'searching' && (
        <ProgressBar
          label="Looking for caches and logs"
          helperText={
            discovery.progress
              ? `${discovery.progress.found.toLocaleString()} found so far`
              : 'Starting'
          }
        />
      )}

      {discovery.status === 'failed' && (
        <InlineNotification
          kind={discovery.error.kind === 'cancelled' ? 'info' : 'error'}
          lowContrast
          hideCloseButton
          title={errorTitle(discovery.error)}
          subtitle={discovery.error.message}
        />
      )}

      {result && (
        <>
          <div role="status" className="cleanup-view__summary">
            {candidates.length === 0 ? (
              <InlineNotification
                kind="info"
                lowContrast
                hideCloseButton
                title="Nothing to clean up"
                subtitle="No cache or log folders were found that are safe to offer right now."
              />
            ) : (
              <p>
                Found <strong>{pluralItems(candidates.length)}</strong>, about{' '}
                <strong>{formatBytes(result.totalBytes)}</strong>. Select what to remove.
              </p>
            )}
          </div>

          {candidates.length > 0 && (
            <>
              {/* Inline toolbar — always visible above the tables */}
              <div className="cleanup-view__bar">
                <Button
                  kind="tertiary"
                  size="md"
                  onClick={scan.selectAll}
                  disabled={selected.size === candidates.length}
                >
                  Select all
                </Button>
                <Button
                  kind="ghost"
                  size="md"
                  onClick={scan.clearSelection}
                  disabled={selected.size === 0}
                >
                  Clear selection
                </Button>
              </div>

              {CATEGORIES.map(([category, info]) => {
                const items = candidates.filter((c) => c.category === category);
                return items.length === 0 ? null : (
                  <CategoryTable
                    key={category}
                    info={info}
                    items={items}
                    selected={selected}
                    disabled={move.step !== 'closed'}
                    onToggle={scan.toggle}
                  />
                );
              })}
            </>
          )}

          <SkippedNotes result={result} />
        </>
      )}

      <CleanupConfirmModal
        state={move}
        onConfirm={(acknowledged) => void scan.confirm(acknowledged)}
        onClose={closeModal}
      />
    </div>
  );
}
