import { createPortal } from 'react-dom';
import { Button } from '@carbon/react';
import FolderOpen from '@carbon/icons-react/es/FolderOpen';
import TrashCan from '@carbon/icons-react/es/TrashCan';
import { formatBytes } from '../format';
import { showInFinder } from '../ipc';

const pluralItems = (count: number) =>
  `${count.toLocaleString()} ${count === 1 ? 'item' : 'items'}`;

export interface SelectionActionBarProps {
  /** Number of selected items. Bar is hidden when 0. */
  count: number;
  /** Total bytes of the selection. */
  bytes: number;
  /** Path to reveal when exactly one item is selected. */
  singlePath?: string;
  /** Display name of the single selected item (used in the tooltip). */
  singleName?: string;
  /** Label for the primary (trash) action button. */
  primaryLabel: string;
  /** Whether the primary action button is disabled. */
  primaryDisabled?: boolean;
  /** Called when the primary action button is clicked. */
  onPrimary: () => void;
  /** Extra action rendered between Show in Finder and the primary button. */
  secondaryAction?: React.ReactNode;
}

export function SelectionActionBar({
  count,
  bytes,
  singlePath,
  singleName,
  primaryLabel,
  primaryDisabled,
  onPrimary,
  secondaryAction,
}: SelectionActionBarProps) {
  const visible = count > 0;

  return createPortal(
    <div
      className={`selection-action-bar${visible ? '' : ' selection-action-bar--hidden'}`}
      aria-live="polite"
      aria-label="Selection actions"
    >
      <span className="selection-action-bar__count">
        {pluralItems(count)} selected, {formatBytes(bytes)}
      </span>
      <div className="selection-action-bar__buttons">
        <Button
          kind="ghost"
          size="md"
          renderIcon={FolderOpen}
          disabled={!singlePath}
          onClick={() => singlePath && void showInFinder(singlePath)}
          title={
            singlePath && singleName
              ? `Show "${singleName}" in Finder`
              : 'Select exactly one item to show in Finder'
          }
        >
          Show in Finder
        </Button>
        {secondaryAction}
        <Button
          kind="danger--tertiary"
          size="md"
          renderIcon={TrashCan}
          disabled={primaryDisabled}
          onClick={onPrimary}
        >
          {primaryLabel}
        </Button>
      </div>
    </div>,
    document.body,
  );
}
