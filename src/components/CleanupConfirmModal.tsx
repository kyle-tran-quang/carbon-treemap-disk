import {
  Checkbox,
  InlineLoading,
  InlineNotification,
  Modal,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Tag,
} from '@carbon/react';
import { useState } from 'react';
import { formatBytes } from '../format';
import type {
  CandidateCleanupResult,
  CleanupError,
  CleanupItemResult,
  CleanupPlan,
  ExcludedItem,
} from '../ipc';
import type { MoveState } from '../state/useCleanupScan';

interface CleanupConfirmModalProps {
  state: MoveState;
  onConfirm: (acknowledged: boolean) => void;
  onClose: () => void;
}

const pluralItems = (count: number) =>
  `${count.toLocaleString()} ${count === 1 ? 'item' : 'items'}`;

function errorTitle(error: CleanupError): string {
  switch (error.kind) {
    case 'stalePlan':
      return 'This preview is out of date';
    case 'noDiscovery':
    case 'unknownCandidate':
      return 'The list is out of date';
    case 'discoveryRunning':
      return 'A search is running';
    case 'tooMany':
      return 'Too many items selected';
    default:
      return 'Could not move items';
  }
}

function ExcludedList({ items }: { items: ExcludedItem[] }) {
  if (items.length === 0) return null;
  return (
    <section className="cleanup__section" aria-label="Items that will not be moved">
      <h4 className="cleanup__heading">Not included ({items.length.toLocaleString()})</h4>
      <p className="cleanup__note">
        These changed, are protected or are no longer there, so they will stay where they
        are.
      </p>
      <Table size="sm" aria-label="Items that will not be moved">
        <TableHead>
          <TableRow>
            <TableHeader>Path</TableHeader>
            <TableHeader>Reason</TableHeader>
          </TableRow>
        </TableHead>
        <TableBody>
          {items.map((item) => (
            <TableRow key={item.node}>
              <TableCell className="cleanup__path">{item.path}</TableCell>
              <TableCell>{item.reason.message}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </section>
  );
}

function PreviewBody({
  plan,
  moving,
  acknowledged,
  onAcknowledge,
}: {
  plan: CleanupPlan;
  moving: boolean;
  acknowledged: boolean;
  onAcknowledge: (value: boolean) => void;
}) {
  const hasItems = plan.items.length > 0;
  return (
    <div className="cleanup">
      {hasItems ? (
        <p className="cleanup__summary">
          <strong>{pluralItems(plan.items.length)}</strong> will be moved to the Trash,
          about <strong>{formatBytes(plan.totalBytes)}</strong>. Nothing has been moved
          yet.
        </p>
      ) : (
        <InlineNotification
          kind="info"
          lowContrast
          hideCloseButton
          title="Nothing can be moved"
          subtitle="Every selected item changed, is protected or is gone."
        />
      )}
      <p className="cleanup__note">
        Items go to the Trash, so you can put them back. Space is freed only after you
        empty the Trash. Apps rebuild these items when they need them, which can make the
        next launch or build slower.
      </p>

      {hasItems && (
        <Table
          size="sm"
          aria-label="Items that will be moved to the Trash, largest first"
        >
          <TableHead>
            <TableRow>
              <TableHeader>Path</TableHeader>
              <TableHeader className="cleanup__number">Size</TableHeader>
            </TableRow>
          </TableHead>
          <TableBody>
            {plan.items.map((item) => (
              <TableRow key={item.node}>
                <TableCell className="cleanup__path">
                  {item.path}
                  {item.kind === 'dir' ? '/' : ''}
                  {item.warning && (
                    <>
                      {' '}
                      <Tag type="red" size="sm" title={item.warning.message}>
                        Outside home folder
                      </Tag>
                    </>
                  )}
                </TableCell>
                <TableCell className="cleanup__number">
                  {formatBytes(item.size)}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}

      <ExcludedList items={plan.excluded} />

      {plan.needsAcknowledgement && hasItems && (
        <div className="cleanup__ack">
          <InlineNotification
            kind="warning"
            lowContrast
            hideCloseButton
            title="Some items are outside your home folder"
            subtitle="They may belong to the system or other users' apps."
          />
          <Checkbox
            id="cleanup-scan-acknowledge"
            labelText="I understand and want to move these items"
            checked={acknowledged}
            disabled={moving}
            onChange={(_, { checked }) => onAcknowledge(checked)}
          />
        </div>
      )}
      {moving && <InlineLoading description="Moving to the Trash" />}
    </div>
  );
}

function resultLabel(result: CleanupItemResult): string {
  switch (result.outcome) {
    case 'moved':
      return 'Moved to the Trash';
    case 'alreadyGone':
      return 'Already gone';
    default:
      return result.message ?? 'Could not be moved';
  }
}

function DoneBody({
  plan,
  result,
}: {
  plan: CleanupPlan;
  result: CandidateCleanupResult;
}) {
  const problems = result.results.filter((r) => r.outcome !== 'moved');
  const allFailed = result.moved === 0 && result.failed > 0;
  const kind = result.failed === 0 ? 'success' : allFailed ? 'error' : 'warning';
  const title =
    result.failed === 0
      ? 'Moved to the Trash'
      : allFailed
        ? 'Nothing was moved'
        : 'Some items were not moved';
  const parts = [
    `${pluralItems(result.moved)} moved (${formatBytes(result.movedBytes)})`,
  ];
  if (result.failed > 0) parts.push(`${result.failed.toLocaleString()} failed`);
  if (result.alreadyGone > 0)
    parts.push(`${result.alreadyGone.toLocaleString()} already gone`);

  return (
    <div className="cleanup">
      <InlineNotification
        kind={kind}
        lowContrast
        hideCloseButton
        title={title}
        subtitle={`${parts.join(', ')}. Empty the Trash to free the space.`}
      />
      {!result.logSaved && (
        <InlineNotification
          kind="warning"
          lowContrast
          hideCloseButton
          title="Cleanup record not saved"
          subtitle="The items were moved, but this cleanup could not be written to the log."
        />
      )}
      {result.moved > 0 && (
        <p className="cleanup__note">
          If you have scanned a folder in the treemap, scan it again to see the new sizes.
        </p>
      )}
      {problems.length > 0 && (
        <Table size="sm" aria-label="Items that were not moved">
          <TableHead>
            <TableRow>
              <TableHeader>Path</TableHeader>
              <TableHeader>Result</TableHeader>
            </TableRow>
          </TableHead>
          <TableBody>
            {problems.map((r) => (
              <TableRow key={r.node}>
                <TableCell className="cleanup__path">{r.path}</TableCell>
                <TableCell>{resultLabel(r)}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      {plan.excluded.length > 0 && (
        <p className="cleanup__note">
          {pluralItems(plan.excluded.length)}{' '}
          {plan.excluded.length === 1 ? 'was' : 'were'} not included because{' '}
          {plan.excluded.length === 1 ? 'it' : 'they'} changed, or{' '}
          {plan.excluded.length === 1 ? 'is' : 'are'} protected or gone.
        </p>
      )}
    </div>
  );
}

/**
 * Owns the acknowledgement checkbox. It is rendered with `key={planId}`, so a
 * new preview always starts unacknowledged.
 */
function PreviewModal({
  plan,
  moving,
  onConfirm,
  onClose,
}: {
  plan: CleanupPlan;
  moving: boolean;
  onConfirm: (acknowledged: boolean) => void;
  onClose: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const blocked = plan.items.length === 0 || (plan.needsAcknowledgement && !acknowledged);
  return (
    <Modal
      open
      danger
      size="md"
      hasScrollingContent
      preventCloseOnClickOutside
      modalLabel="Clean up caches"
      modalHeading="Move to Trash"
      primaryButtonText="Move to Trash"
      secondaryButtonText="Cancel"
      primaryButtonDisabled={blocked || moving}
      selectorPrimaryFocus=".cds--btn--secondary"
      onRequestClose={onClose}
      onSecondarySubmit={onClose}
      onRequestSubmit={() => onConfirm(acknowledged)}
    >
      <PreviewBody
        plan={plan}
        moving={moving}
        acknowledged={acknowledged}
        onAcknowledge={setAcknowledged}
      />
    </Modal>
  );
}

export function CleanupConfirmModal({
  state,
  onConfirm,
  onClose,
}: CleanupConfirmModalProps) {
  if (state.step === 'closed') return null;

  if (state.step === 'preview' || state.step === 'moving') {
    return (
      <PreviewModal
        key={state.plan.planId}
        plan={state.plan}
        moving={state.step === 'moving'}
        onConfirm={onConfirm}
        onClose={onClose}
      />
    );
  }

  return (
    <Modal
      open
      passiveModal
      size="md"
      hasScrollingContent
      modalLabel="Clean up caches"
      modalHeading={
        state.step === 'planning'
          ? 'Preparing preview'
          : state.step === 'done'
            ? 'Cleanup result'
            : errorTitle(state.error)
      }
      onRequestClose={onClose}
    >
      {state.step === 'planning' && (
        <InlineLoading description="Checking the selection" />
      )}
      {state.step === 'done' && <DoneBody plan={state.plan} result={state.result} />}
      {state.step === 'error' && (
        <InlineNotification
          kind="error"
          lowContrast
          hideCloseButton
          title={errorTitle(state.error)}
          subtitle={state.error.message}
        />
      )}
    </Modal>
  );
}
