import { useEffect, useRef } from 'react';
import {
  Breadcrumb,
  BreadcrumbItem,
  Button,
  InlineLoading,
  InlineNotification,
} from '@carbon/react';
import Delete from '@carbon/icons-react/es/TrashCan';
import { formatBytes } from '../format';
import type { useExplorer } from '../state/useExplorer';
import { useScrollMemory } from '../state/useScrollMemory';
import { TreemapCanvas } from '../treemap/TreemapCanvas';
import { baseName } from '../treemap/paths';
import type { SelectedItem } from '../treemap/selection';
import { FolderTable } from './FolderTable';

type Explorer = ReturnType<typeof useExplorer>;

interface TreemapExplorerProps {
  explorer: Explorer;
  theme: string;
  /** Opens the dry-run preview for the current selection. */
  onCleanup: () => void;
  cleanupDisabled: boolean;
}

export function TreemapExplorer({
  explorer,
  theme,
  onCleanup,
  cleanupDisabled,
}: TreemapExplorerProps) {
  const { current, error, loading, selectedIds, selectionSummary } = explorer;
  useScrollMemory(error ? null : (current?.path ?? null));

  // Opening a folder replaces the focused row or link. Without this, keyboard
  // focus drops to the page and a screen reader says nothing about the change.
  const headingRef = useRef<HTMLHeadingElement>(null);
  const shownPath = current?.path ?? null;
  const previousPath = useRef<string | null>(null);
  useEffect(() => {
    const before = previousPath.current;
    previousPath.current = shownPath;
    if (before === null || shownPath === null || before === shownPath) return;
    const active = document.activeElement;
    if (!active || active === document.body) headingRef.current?.focus();
  }, [shownPath]);

  if (error) {
    return (
      <InlineNotification
        kind="error"
        lowContrast
        hideCloseButton
        title="Could not load this folder"
        subtitle={error}
      />
    );
  }
  if (!current) {
    return loading ? <InlineLoading description="Loading treemap" /> : null;
  }

  const { response, path } = current;
  const { view, breadcrumbs } = response;
  const lastIndex = breadcrumbs.length - 1;
  const folderName =
    lastIndex === 0 ? baseName(breadcrumbs[0].name) : breadcrumbs[lastIndex].name;

  const toggle = (item: SelectedItem) => explorer.select(item, true);

  return (
    <section className="explorer" aria-label="Disk usage">
      <h2 ref={headingRef} tabIndex={-1} className="cds--visually-hidden">
        Showing {folderName}, {formatBytes(view.size)} in {view.files.toLocaleString()}{' '}
        files
      </h2>
      <div className="explorer__bar">
        <Breadcrumb noTrailingSlash aria-label="Folder path">
          {breadcrumbs.map((crumb, index) => {
            const label = index === 0 ? baseName(crumb.name) : crumb.name;
            return index === lastIndex ? (
              <BreadcrumbItem key={crumb.id} isCurrentPage>
                {label}
              </BreadcrumbItem>
            ) : (
              <BreadcrumbItem key={crumb.id}>
                <a
                  href="#"
                  title={index === 0 ? crumb.name : undefined}
                  onClick={(event) => {
                    event.preventDefault();
                    void explorer.open(crumb.id);
                  }}
                >
                  {label}
                </a>
              </BreadcrumbItem>
            );
          })}
        </Breadcrumb>
        <div className="explorer__total">
          <span>
            {formatBytes(view.size)} in {view.files.toLocaleString()} files
          </span>
          {loading && <InlineLoading description="Loading" />}
        </div>
      </div>

      <TreemapCanvas
        view={view}
        path={path}
        theme={theme}
        selectedIds={selectedIds}
        onSelect={explorer.select}
        onOpen={(id) => void explorer.open(id)}
      />
      <p className="explorer__hint">
        Click to select. Shift or command click to add to the selection. Double-click a
        folder to open it.
      </p>

      <div className="explorer__selection">
        <span role="status" aria-live="polite">
          {selectionSummary.count > 0
            ? `${selectionSummary.count.toLocaleString()} ${
                selectionSummary.count === 1 ? 'item' : 'items'
              } selected, ${formatBytes(selectionSummary.size)}`
            : 'Nothing selected'}
        </span>
        {selectionSummary.count > 0 && (
          <>
            <Button kind="ghost" size="sm" onClick={explorer.clearSelection}>
              Clear selection
            </Button>
            <Button
              kind="danger--tertiary"
              size="sm"
              renderIcon={Delete}
              disabled={cleanupDisabled}
              onClick={onCleanup}
            >
              Preview move to Trash
            </Button>
          </>
        )}
      </div>

      <FolderTable
        view={view}
        path={path}
        selectedIds={selectedIds}
        onToggle={toggle}
        onToggleAll={explorer.setMany}
        onOpen={(id) => void explorer.open(id)}
      />
    </section>
  );
}
