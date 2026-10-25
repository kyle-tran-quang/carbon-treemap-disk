import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Accordion,
  AccordionItem,
  Button,
  Checkbox,
  InlineNotification,
  ProgressBar,
  RadioTile,
  Search as SearchInput,
  Select,
  SelectItem,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Tag,
  TileGroup,
  Toggle,
} from '@carbon/react';
import Close from '@carbon/icons-react/es/Close';
import FolderAdd from '@carbon/icons-react/es/FolderAdd';
import FolderOpen from '@carbon/icons-react/es/FolderOpen';
import Renew from '@carbon/icons-react/es/Renew';
import Search from '@carbon/icons-react/es/Search';
import View from '@carbon/icons-react/es/View';
import { formatBytes } from '../format';
import {
  pickFolder,
  previewFile,
  showInFinder,
  type CleanupError,
  type DuplicateGroup,
  type DuplicateProgress,
  type DuplicateSort,
} from '../ipc';
import {
  DEFAULT_MIN_SIZE_BYTES,
  type Duplicates,
  type DuplicateSelection,
} from '../state/useDuplicates';
import { DuplicateConfirmModal } from './DuplicateConfirmModal';
import { ScanIssueList } from './ScanIssueList';

interface DuplicatesViewProps {
  duplicates: Duplicates;
  recentRoots: readonly string[];
  onFolderScanned: (folder: string) => void;
}

const MIN_SIZE_CHOICES: readonly [number, string][] = [
  [100 * 1024, '100 KB or larger'],
  [DEFAULT_MIN_SIZE_BYTES, '1 MB or larger (recommended)'],
  [10 * 1024 * 1024, '10 MB or larger'],
  [100 * 1024 * 1024, '100 MB or larger'],
];

const SORT_CHOICES: readonly [DuplicateSort, string][] = [
  ['wastedSpace', 'Most space to reclaim'],
  ['fileSize', 'Largest files'],
  ['copyCount', 'Most copies'],
];

const pluralFiles = (count: number) =>
  `${count.toLocaleString()} ${count === 1 ? 'file' : 'files'}`;

const pluralGroups = (count: number) =>
  `${count.toLocaleString()} ${count === 1 ? 'group' : 'groups'}`;

function formatDate(secs: number): string {
  return secs > 0 ? new Date(secs * 1000).toLocaleDateString() : 'Unknown';
}

function errorTitle(error: CleanupError): string {
  switch (error.kind) {
    case 'cancelled':
      return 'Scan cancelled';
    case 'invalidRoot':
      return 'Choose a different folder';
    case 'permissionDenied':
      return 'Permission denied';
    case 'duplicateScanRunning':
      return 'A scan is already running';
    default:
      return 'Could not find duplicates';
  }
}

/** The stage name only. The progress bar announces this text, so it must not change with every count. */
function stageName(progress: DuplicateProgress | null): string {
  switch (progress?.stage) {
    case 'listing':
      return 'Listing files';
    case 'fingerprinting':
      return 'Comparing file samples';
    case 'hashing':
      return 'Confirming matches';
    default:
      return 'Starting';
  }
}

/** The running counts. Shown, but not announced. */
function stageCounts(progress: DuplicateProgress | null): string {
  if (!progress) return '';
  const done = progress.done.toLocaleString();
  switch (progress.stage) {
    case 'listing':
      return `${done} files so far`;
    case 'fingerprinting':
      return `${done} of ${progress.total.toLocaleString()} files`;
    default:
      return `${done} of ${progress.total.toLocaleString()} files, ${formatBytes(progress.bytes)} read`;
  }
}

function progressValue(progress: DuplicateProgress | null): number | undefined {
  if (!progress || progress.total === 0 || progress.stage === 'listing') return undefined;
  return Math.min(100, Math.round((progress.done / progress.total) * 100));
}

function FolderList({
  label,
  folders,
  disabled,
  onRemove,
}: {
  label: string;
  folders: readonly string[];
  disabled: boolean;
  onRemove: (folder: string) => void;
}) {
  if (folders.length === 0) return null;
  return (
    <ul className="duplicates__folders" aria-label={label}>
      {folders.map((folder) => (
        <li key={folder} className="duplicates__folder">
          <span className="cleanup__path">{folder}</span>
          <Button
            kind="ghost"
            size="sm"
            hasIconOnly
            renderIcon={Close}
            iconDescription={`Remove ${folder}`}
            tooltipPosition="left"
            disabled={disabled}
            onClick={() => onRemove(folder)}
          />
        </li>
      ))}
    </ul>
  );
}

function StartPanel({ duplicates, recentRoots, onFolderScanned }: DuplicatesViewProps) {
  const { options, setOptions } = duplicates;
  const scanning = duplicates.scan.status === 'scanning';
  const needsFolder = options.scope === 'folder' && !options.folder;

  const chooseFolder = async () => {
    const folder = await pickFolder();
    if (folder) setOptions({ scope: 'folder', folder });
  };

  const addProtected = async () => {
    const folder = await pickFolder();
    if (folder && !options.protectedRoots.includes(folder)) {
      setOptions({ protectedRoots: [...options.protectedRoots, folder] });
    }
  };

  const begin = () => {
    if (options.scope === 'folder' && options.folder) onFolderScanned(options.folder);
    void duplicates.start();
  };

  const exclusion = (
    key: keyof typeof options.exclusions,
    labelText: string,
    helperText: string,
  ) => (
    <Checkbox
      id={`duplicates-skip-${key}`}
      labelText={labelText}
      helperText={helperText}
      checked={options.exclusions[key]}
      disabled={scanning}
      onChange={(_, { checked }) =>
        setOptions({ exclusions: { ...options.exclusions, [key]: checked } })
      }
    />
  );

  return (
    <section className="duplicates__start" aria-label="Scan options">
      <TileGroup
        legend="What to scan"
        name="duplicates-scope"
        valueSelected={options.scope}
        onChange={(value) =>
          setOptions({ scope: value === 'folder' ? 'folder' : 'home' })
        }
      >
        <RadioTile id="duplicates-scope-home" value="home" disabled={scanning}>
          <strong>Home folder (recommended)</strong>
          <br />
          Your documents, downloads, photos and projects. System files are never scanned.
        </RadioTile>
        <RadioTile id="duplicates-scope-folder" value="folder" disabled={scanning}>
          <strong>A folder…</strong>
          <br />
          {options.folder ? (
            <span className="cleanup__path">{options.folder}</span>
          ) : (
            'Scan one folder and everything inside it.'
          )}
        </RadioTile>
      </TileGroup>

      {options.scope === 'folder' && (
        <div className="duplicates__row">
          <Button
            kind="tertiary"
            renderIcon={FolderOpen}
            disabled={scanning}
            onClick={() => void chooseFolder()}
          >
            {options.folder ? 'Choose a different folder' : 'Choose folder'}
          </Button>
          {recentRoots.length > 0 && (
            <ul className="app__recent-list" aria-label="Recent folders">
              {recentRoots.map((root) => (
                <li key={root}>
                  <Tag
                    as="button"
                    type="gray"
                    title={root}
                    aria-label={`Use ${root}`}
                    onClick={() => setOptions({ scope: 'folder', folder: root })}
                  >
                    {root}
                  </Tag>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <div className="duplicates__protect">
        <h2 className="duplicates__heading">Protect a folder</h2>
        <p className="cleanup__note">
          Files in a protected folder are always kept and can never be moved. Use it for
          your originals, such as your photo library or a backup. Copies elsewhere can
          still be moved.
        </p>
        <FolderList
          label="Protected folders"
          folders={options.protectedRoots}
          disabled={scanning}
          onRemove={(folder) =>
            setOptions({
              protectedRoots: options.protectedRoots.filter((f) => f !== folder),
            })
          }
        />
        <Button
          kind="ghost"
          renderIcon={FolderAdd}
          disabled={scanning}
          onClick={() => void addProtected()}
        >
          Protect a folder
        </Button>
      </div>

      <Accordion>
        <AccordionItem title="Options">
          <div className="duplicates__options">
            <Select
              id="duplicates-min-size"
              labelText="Smallest file to look at"
              helperText="Small files rarely free much space and there are many of them."
              value={String(options.minSizeBytes)}
              disabled={scanning}
              onChange={(event) =>
                setOptions({ minSizeBytes: Number(event.target.value) })
              }
            >
              {MIN_SIZE_CHOICES.map(([bytes, label]) => (
                <SelectItem key={bytes} value={String(bytes)} text={label} />
              ))}
            </Select>
            <fieldset className="duplicates__fieldset">
              <legend>Folders to skip</legend>
              {exclusion(
                'packages',
                'App bundles and other packages',
                'Their files are part of the app. Moving one can break it.',
              )}
              {exclusion(
                'nodeModules',
                'node_modules',
                'Dependency folders are full of identical files on purpose.',
              )}
              {exclusion('git', '.git folders', 'Git stores its own objects there.')}
              {exclusion('library', 'Library folder', 'Apps keep their own data there.')}
              {exclusion('trash', 'Trash', 'Files already in the Trash.')}
            </fieldset>
            <Toggle
              id="duplicates-cloud"
              size="sm"
              labelText="Cloud-synced folders"
              labelA="Skipped"
              labelB="Included (can be slow)"
              toggled={options.includeCloud}
              disabled={scanning}
              onToggle={(value) => setOptions({ includeCloud: value })}
            />
          </div>
        </AccordionItem>
      </Accordion>

      <div className="app__scan-actions">
        <Button
          kind="primary"
          renderIcon={Search}
          disabled={scanning || needsFolder}
          onClick={begin}
        >
          Find duplicates
        </Button>
      </div>
      <p className="cleanup__note">
        Finding duplicates only reads files. Nothing is moved until you review a list and
        confirm, and at least one copy of every file always stays.
      </p>
    </section>
  );
}

function GroupRows({
  group,
  chosen,
  disabled,
  onToggle,
}: {
  group: DuplicateGroup;
  chosen: ReadonlySet<number> | undefined;
  disabled: boolean;
  onToggle: (fileId: number, checked: boolean) => void;
}) {
  const chosenCount = chosen?.size ?? 0;
  // One copy must always stay, so the last unchosen copy cannot be chosen.
  const lastUnchosen = chosenCount >= group.files.length - 1;
  return (
    <>
      {group.keepReasonText && (
        <p className="cleanup__note">Suggested copy to keep: {group.keepReasonText}</p>
      )}
      <Table size="sm" aria-label={`Copies of ${group.files[0]?.name ?? 'this file'}`}>
        <TableHead>
          <TableRow>
            <TableHeader>Move to Trash</TableHeader>
            <TableHeader>Location</TableHeader>
            <TableHeader className="cleanup__number">Last changed</TableHeader>
            <TableHeader>
              <span className="duplicates__visually-hidden">Actions</span>
            </TableHeader>
          </TableRow>
        </TableHead>
        <TableBody>
          {group.files.map((file) => {
            const isChosen = chosen?.has(file.id) ?? false;
            const lastCopy = !isChosen && lastUnchosen;
            // A disabled control cannot be focused, so the reason is part of
            // its name for screen readers and also shown as a tooltip.
            const why = file.protected
              ? 'Protected folder: this copy is always kept'
              : lastCopy
                ? 'At least one copy must stay'
                : undefined;
            return (
              <TableRow key={file.id}>
                <TableCell>
                  <Checkbox
                    id={`duplicate-file-${file.id}`}
                    labelText={`Move ${file.path} to the Trash${why ? `. ${why}` : ''}`}
                    hideLabel
                    title={why}
                    checked={isChosen}
                    disabled={disabled || file.protected || lastCopy}
                    onChange={(_, { checked }) => onToggle(file.id, checked)}
                  />
                </TableCell>
                <TableCell className="cleanup__path">
                  <button
                    className="duplicates__path-button"
                    title={`Quick Look: ${file.path}`}
                    onClick={() => void previewFile(file.path)}
                  >
                    {file.folder}
                    {file.name && <strong>/{file.name}</strong>}
                  </button>{' '}
                  {file.suggestedKeep && (
                    <Tag type="green" size="sm">
                      Suggested to keep
                    </Tag>
                  )}
                  {file.protected && (
                    <Tag type="gray" size="sm">
                      Protected
                    </Tag>
                  )}
                </TableCell>
                <TableCell className="cleanup__number">
                  {formatDate(file.modifiedSecs)}
                </TableCell>
                <TableCell>
                  <Button
                    kind="ghost"
                    size="sm"
                    hasIconOnly
                    renderIcon={View}
                    iconDescription="Quick Look"
                    tooltipPosition="left"
                    onClick={() => void previewFile(file.path)}
                  />
                  <Button
                    kind="ghost"
                    size="sm"
                    hasIconOnly
                    renderIcon={FolderOpen}
                    iconDescription="Show in Finder"
                    tooltipPosition="left"
                    onClick={() => void showInFinder(file.path)}
                  />
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </>
  );
}

function matches(group: DuplicateGroup, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return group.files.some((f) => f.path.toLowerCase().includes(needle));
}

function Results({
  duplicates,
  selection,
}: {
  duplicates: Duplicates;
  selection: DuplicateSelection;
}) {
  const { scan, listing } = duplicates;
  const [query, setQuery] = useState('');
  const summary = scan.status === 'ready' ? scan.summary : null;
  const visible = useMemo(
    () => listing.groups.filter((g) => matches(g, query)),
    [listing.groups, query],
  );
  const locked = duplicates.move.step !== 'closed';
  if (!summary) return null;

  const hidden = listing.totalGroups - listing.groups.length;
  return (
    <>
      <div className="cleanup-view__summary">
        {listing.totalGroups === 0 ? (
          <InlineNotification
            kind="info"
            lowContrast
            hideCloseButton
            title="No duplicates found"
            subtitle={`No files of ${formatBytes(summary.minSizeBytes)} or more have identical copies in ${summary.root}.`}
          />
        ) : (
          <p>
            Found <strong>{pluralGroups(listing.totalGroups)}</strong> of identical files.
            About <strong>{formatBytes(listing.reclaimableBytes)}</strong> could be freed
            by keeping one copy of each.
          </p>
        )}
      </div>

      <p className="cleanup__note">
        Scanned {summary.filesListed.toLocaleString()} files in {summary.root}.
        {summary.protectedRoots.length > 0 &&
          ` Protected: ${summary.protectedRoots.join(', ')}.`}
        {summary.hardLinksCollapsed > 0 &&
          ` ${summary.hardLinksCollapsed.toLocaleString()} files that are hard links to the same data count as one, because moving one would free nothing.`}
      </p>
      {summary.skippedCloudFolders.length > 0 && (
        <InlineNotification
          kind="info"
          lowContrast
          hideCloseButton
          title="Cloud-synced folders were skipped"
          subtitle={`${summary.skippedCloudFolders.join(', ')}. Turn on "Cloud-synced folders" in the options and scan again to include them.`}
        />
      )}
      {summary.issueCount > summary.issues.length && (
        <p className="cleanup__note">
          {summary.issueCount.toLocaleString()} items could not be read. The first{' '}
          {summary.issues.length.toLocaleString()} are listed below.
        </p>
      )}
      <ScanIssueList issues={summary.issues} />

      {listing.totalGroups > 0 && (
        <>
          <div className="duplicates__toolbar">
            <Button
              kind="tertiary"
              disabled={locked}
              onClick={() => void duplicates.selectSuggested()}
            >
              Select suggested duplicates
            </Button>
            <Button
              kind="ghost"
              disabled={locked || selection.count === 0}
              onClick={duplicates.clearSelection}
            >
              Clear selection
            </Button>
            <Select
              id="duplicates-sort"
              labelText="Sort by"
              size="md"
              inline
              value={duplicates.sort}
              onChange={(event) =>
                duplicates.setSort(event.target.value as DuplicateSort)
              }
            >
              {SORT_CHOICES.map(([value, label]) => (
                <SelectItem key={value} value={value} text={label} />
              ))}
            </Select>
            <SearchInput
              id="duplicates-search"
              size="md"
              labelText="Filter by name or folder"
              placeholder="Filter by name or folder"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onClear={() => setQuery('')}
            />
          </div>
          <p className="cleanup__note">
            Nothing is chosen until you pick copies or select the suggestions. The
            suggestion keeps one copy in each group, preferring the oldest one that is not
            in Downloads or the Trash and does not look like a copy.
          </p>
          {duplicates.suggestionTruncated && (
            <InlineNotification
              kind="info"
              lowContrast
              hideCloseButton
              title="Suggestion limited"
              subtitle="One cleanup moves at most 1,000 files, so only the groups with the most space to reclaim were selected. Run it again afterwards for the rest."
            />
          )}

          {visible.length === 0 ? (
            <p className="cleanup__note">
              No loaded group matches “{query}”.
              {hidden > 0 && ' Load more groups to search further.'}
            </p>
          ) : (
            <Accordion className="duplicates__groups" align="start">
              {visible.map((group) => {
                const chosen = selection.groups.get(group.id);
                return (
                  <AccordionItem
                    key={group.id}
                    title={
                      <span className="duplicates__group-title">
                        <strong>{group.files[0]?.name ?? 'Identical files'}</strong>{' '}
                        <span className="cleanup-view__category-total">
                          {pluralFiles(group.files.length)}, {formatBytes(group.size)}{' '}
                          each, {formatBytes(group.reclaimableBytes)} to reclaim
                          {chosen ? `, ${chosen.size} chosen` : ''}
                        </span>
                      </span>
                    }
                  >
                    <GroupRows
                      group={group}
                      chosen={chosen}
                      disabled={locked}
                      onToggle={(fileId, checked) =>
                        duplicates.toggle(group.id, fileId, group.size, checked)
                      }
                    />
                  </AccordionItem>
                );
              })}
            </Accordion>
          )}
          {hidden > 0 && (
            <Button
              kind="tertiary"
              disabled={duplicates.loadingGroups}
              onClick={duplicates.loadMore}
            >
              Load more groups ({hidden.toLocaleString()} left)
            </Button>
          )}
        </>
      )}
    </>
  );
}

/** What a screen reader hears when the scan state changes. */
function announcement(duplicates: Duplicates): string {
  const { scan, listing } = duplicates;
  switch (scan.status) {
    case 'scanning':
      return 'Scan started.';
    case 'failed':
      return `${errorTitle(scan.error)}. ${scan.error.message}`;
    case 'ready':
      return listing.totalGroups === 0
        ? 'Scan finished. No duplicates found.'
        : `Scan finished. Found ${pluralGroups(listing.totalGroups)}, about ${formatBytes(listing.reclaimableBytes)} that could be freed.`;
    default:
      return '';
  }
}

export function DuplicatesView(props: DuplicatesViewProps) {
  const { duplicates } = props;
  const { scan, selection } = duplicates;
  const headingRef = useRef<HTMLHeadingElement>(null);
  const showStart = scan.status === 'idle' || scan.status === 'failed';

  // The control that had focus (Cancel, New scan, Find duplicates) is replaced
  // when the screen changes, so move focus to the heading instead of losing it.
  const shownStatus = useRef(scan.status);
  useEffect(() => {
    if (shownStatus.current === scan.status) return;
    shownStatus.current = scan.status;
    headingRef.current?.focus();
  }, [scan.status]);

  // Announced once the totals are known, not when the scan state flips to
  // ready, because the first page of groups arrives a moment later.
  const loaded = scan.status !== 'ready' || !duplicates.loadingGroups;
  const message = loaded ? announcement(duplicates) : '';

  // After the dialog closes, restore focus to the page heading.
  const closeModal = () => {
    duplicates.close();
    requestAnimationFrame(() => headingRef.current?.focus());
  };

  return (
    <div className="cleanup-view">
      <h1 ref={headingRef} tabIndex={-1}>
        Find duplicates
      </h1>
      <p>
        Find files with identical content and move the extra copies to the Trash. Files
        are compared by their content, not their names, so a match is a true copy. Items
        go to the Trash, so you can put them back.
      </p>

      {scan.status === 'failed' && (
        <InlineNotification
          kind={scan.error.kind === 'cancelled' ? 'info' : 'error'}
          lowContrast
          hideCloseButton
          title={errorTitle(scan.error)}
          subtitle={scan.error.message}
        />
      )}

      <div role="status" className="duplicates__visually-hidden">
        {message}
      </div>

      {showStart && <StartPanel {...props} />}

      {scan.status === 'scanning' && (
        <section aria-label="Scan progress" className="duplicates__progress">
          <ProgressBar
            label="Looking for duplicates"
            helperText={stageName(scan.progress)}
            value={progressValue(scan.progress)}
            max={100}
          />
          {scan.progress && (
            <p className="cleanup__note cleanup__path">
              {stageCounts(scan.progress)}. {scan.progress.currentPath}
            </p>
          )}
          <div className="app__scan-actions">
            <Button kind="secondary" onClick={duplicates.cancelScan}>
              Cancel scan
            </Button>
          </div>
        </section>
      )}

      {scan.status === 'ready' && (
        <>
          <div className="app__scan-actions">
            <Button
              kind="ghost"
              renderIcon={Renew}
              disabled={duplicates.move.step !== 'closed'}
              onClick={duplicates.reset}
            >
              New scan
            </Button>
          </div>
          <Results duplicates={duplicates} selection={selection} />
        </>
      )}

      <DuplicateConfirmModal
        state={duplicates.move}
        onConfirm={(acknowledged) => void duplicates.confirm(acknowledged)}
        onClose={closeModal}
      />
    </div>
  );
}
