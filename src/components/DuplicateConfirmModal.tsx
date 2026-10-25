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
import type { CleanupError, DuplicateCleanupResult, DuplicatePlan } from '../ipc';
import type { DuplicateMoveState } from '../state/useDuplicates';

interface DuplicateConfirmModalProps {
  state: DuplicateMoveState;
  onConfirm: (acknowledged: boolean) => void;
  onClose: () => void;
}

const pluralFiles = (count: number) =>
  `${count.toLocaleString()} ${count === 1 ? 'file' : 'files'}`;

function errorTitle(error: CleanupError): string {
  switch (error.kind) {
    case 'stalePlan':
      return 'This preview is out of date';
    case 'noDuplicateScan':
      return 'The results are out of date';
    case 'duplicateScanRunning':
      return 'A scan is running';
    case 'lastCopy':
      return 'Every copy was selected';
    case 'protectedFile':
      return 'A protected file was selected';
    case 'tooMany':
      return 'Too many files selected';
    default:
      return 'Could not move files';
  }
}

function PreviewBody({
  plan,
  moving,
  acknowledged,
  onAcknowledge,
}: {
  plan: DuplicatePlan;
  moving: boolean;
  acknowledged: boolean;
  onAcknowledge: (value: boolean) => void;
}) {
  const hasMoves = plan.groups.length > 0;
  return (
    <div className="cleanup">
      {hasMoves ? (
        <p className="cleanup__summary">
          <strong>{pluralFiles(plan.totalFiles)}</strong> will be moved to the Trash,
          about <strong>{formatBytes(plan.totalBytes)}</strong>. In every group below, at
          least one copy stays where it is. Nothing has been moved yet.
        </p>
      ) : (
        <InlineNotification
          kind="info"
          lowContrast
          hideCloseButton
          title="Nothing can be moved"
          subtitle="Every selected file changed, is protected or is gone."
        />
      )}
      <p className="cleanup__note">
        Files go to the Trash, so you can put them back. Space is freed only after you
        empty the Trash.
      </p>

      {plan.groups.map((group) => (
        <section
          key={group.group}
          className="duplicates__plan-group"
          aria-label={`${group.moves.length} of ${group.moves.length + group.kept.length} copies of ${
            group.kept[0]?.name ?? group.moves[0]?.name ?? 'a file'
          }`}
        >
          <h4 className="cleanup__heading">
            {group.kept[0]?.name ?? group.moves[0]?.name}{' '}
            <span className="cleanup-view__category-total">
              {formatBytes(group.size)} each
            </span>
          </h4>
          <Table size="sm" aria-label="Copies in this group">
            <TableHead>
              <TableRow>
                <TableHeader>What happens</TableHeader>
                <TableHeader>Path</TableHeader>
              </TableRow>
            </TableHead>
            <TableBody>
              {group.kept.map((copy) => (
                <TableRow key={`keep-${copy.id}`}>
                  <TableCell>
                    <Tag type="green" size="sm">
                      Stays
                    </Tag>
                    {copy.protected && (
                      <>
                        {' '}
                        <Tag type="gray" size="sm">
                          Protected
                        </Tag>
                      </>
                    )}
                  </TableCell>
                  <TableCell className="cleanup__path">{copy.path}</TableCell>
                </TableRow>
              ))}
              {group.moves.map((item) => (
                <TableRow key={`move-${item.node}`}>
                  <TableCell>
                    <Tag type="red" size="sm">
                      Moves to Trash
                    </Tag>
                    {item.warning && (
                      <>
                        {' '}
                        <Tag type="magenta" size="sm" title={item.warning.message}>
                          Outside home folder
                        </Tag>
                      </>
                    )}
                  </TableCell>
                  <TableCell className="cleanup__path">{item.path}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </section>
      ))}

      {plan.excluded.length > 0 && (
        <section className="cleanup__section" aria-label="Files that will not be moved">
          <h4 className="cleanup__heading">Not included ({plan.excluded.length})</h4>
          <p className="cleanup__note">
            These changed, are protected or are no longer there, so they stay where they
            are.
          </p>
          <Table size="sm" aria-label="Files that will not be moved">
            <TableHead>
              <TableRow>
                <TableHeader>Path</TableHeader>
                <TableHeader>Reason</TableHeader>
              </TableRow>
            </TableHead>
            <TableBody>
              {plan.excluded.map((item) => (
                <TableRow key={item.node}>
                  <TableCell className="cleanup__path">{item.path}</TableCell>
                  <TableCell>{item.reason.message}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </section>
      )}

      {plan.needsAcknowledgement && hasMoves && (
        <div className="cleanup__ack">
          <InlineNotification
            kind="warning"
            lowContrast
            hideCloseButton
            title="Some files are outside your home folder"
            subtitle="They may belong to the system or other users' apps."
          />
          <Checkbox
            id="duplicates-acknowledge"
            labelText="I understand and want to move these files"
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

function DoneBody({ result }: { result: DuplicateCleanupResult }) {
  const problems = result.results.filter((r) => r.outcome !== 'moved');
  const allFailed = result.moved === 0 && result.failed > 0;
  const kind = result.failed === 0 ? 'success' : allFailed ? 'error' : 'warning';
  const title =
    result.failed === 0
      ? 'Moved to the Trash'
      : allFailed
        ? 'Nothing was moved'
        : 'Some files were not moved';
  const parts = [
    `${pluralFiles(result.moved)} moved (${formatBytes(result.movedBytes)})`,
  ];
  if (result.failed > 0) parts.push(`${result.failed.toLocaleString()} failed`);
  if (result.alreadyGone > 0) {
    parts.push(`${result.alreadyGone.toLocaleString()} already gone`);
  }

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
          subtitle="The files were moved, but this cleanup could not be written to the log."
        />
      )}
      {result.remainingGroups > 0 && (
        <p className="cleanup__note">
          {result.remainingGroups.toLocaleString()}{' '}
          {result.remainingGroups === 1 ? 'group is' : 'groups are'} left, with about{' '}
          {formatBytes(result.remainingReclaimableBytes)} that could still be reclaimed.
        </p>
      )}
      {problems.length > 0 && (
        <Table size="sm" aria-label="Files that were not moved">
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
                <TableCell>
                  {r.outcome === 'alreadyGone'
                    ? 'Already gone'
                    : (r.message ?? 'Could not be moved')}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
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
  plan: DuplicatePlan;
  moving: boolean;
  onConfirm: (acknowledged: boolean) => void;
  onClose: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const blocked =
    plan.groups.length === 0 || (plan.needsAcknowledgement && !acknowledged);
  return (
    <Modal
      open
      danger
      size="md"
      hasScrollingContent
      preventCloseOnClickOutside
      modalLabel="Find duplicates"
      modalHeading="Move duplicates to Trash"
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

export function DuplicateConfirmModal({
  state,
  onConfirm,
  onClose,
}: DuplicateConfirmModalProps) {
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
      modalLabel="Find duplicates"
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
      {state.step === 'done' && <DoneBody result={state.result} />}
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
