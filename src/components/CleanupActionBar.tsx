import { type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { Button } from '@carbon/react';
import FolderOpen from '@carbon/icons-react/es/FolderOpen';
import TrashCan from '@carbon/icons-react/es/TrashCan';
import { formatBytes } from '../format';
import type { Candidate } from '../ipc';
import { showInFinder } from '../ipc';
import type { MoveState } from '../state/useCleanupScan';

const pluralItems = (count: number) =>
  `${count.toLocaleString()} ${count === 1 ? 'item' : 'items'}`;

interface CleanupActionBarProps {
  candidates: Candidate[];
  selected: ReadonlySet<number>;
  move: MoveState;
  reviewRef: RefObject<HTMLButtonElement | null>;
  onBegin: () => void;
}

export function CleanupActionBar({
  candidates,
  selected,
  move,
  reviewRef,
  onBegin,
}: CleanupActionBarProps) {
  const visible = candidates.length > 0 && selected.size > 0;

  const selectedBytes = candidates
    .filter((c) => selected.has(c.id))
    .reduce((sum, c) => sum + c.size, 0);

  const singleSelected =
    selected.size === 1 ? candidates.find((c) => selected.has(c.id)) : undefined;

  return createPortal(
    <div
      className={`cleanup-action-bar${visible ? '' : ' cleanup-action-bar--hidden'}`}
      aria-live="polite"
      aria-label="Selection actions"
    >
      <span className="cleanup-action-bar__count">
        {pluralItems(selected.size)} selected, {formatBytes(selectedBytes)}
      </span>
      <div className="cleanup-action-bar__buttons">
        <Button
          kind="ghost"
          size="md"
          renderIcon={FolderOpen}
          disabled={!singleSelected}
          onClick={() => singleSelected && void showInFinder(singleSelected.path)}
          title={
            singleSelected
              ? `Show "${singleSelected.name}" in Finder`
              : 'Select exactly one item to show in Finder'
          }
        >
          Show in Finder
        </Button>
        <Button
          ref={reviewRef}
          kind="danger--tertiary"
          size="md"
          renderIcon={TrashCan}
          disabled={selected.size === 0 || move.step !== 'closed'}
          onClick={onBegin}
        >
          Review and move to Trash
        </Button>
      </div>
    </div>,
    document.body,
  );
}
