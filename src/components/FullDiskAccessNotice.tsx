import { useCallback, useEffect, useState } from 'react';
import { ActionableNotification } from '@carbon/react';
import {
  checkFullDiskAccess,
  openFullDiskAccessSettings,
  type FullDiskAccessStatus,
} from '../ipc';

/**
 * Warns when Full Disk Access is missing. Re-checks when the window regains
 * focus, so the notice clears after the user returns from System Settings.
 * macOS usually applies the grant only after the app restarts, which the
 * message says.
 */
export function FullDiskAccessNotice() {
  const [status, setStatus] = useState<FullDiskAccessStatus>('unknown');
  const [openFailed, setOpenFailed] = useState(false);

  const refresh = useCallback(() => {
    checkFullDiskAccess()
      .then(setStatus)
      .catch(() => setStatus('unknown'));
  }, []);

  useEffect(() => {
    refresh();
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, [refresh]);

  if (status !== 'missing') return null;

  const openSettings = () => {
    setOpenFailed(false);
    openFullDiskAccessSettings().catch(() => setOpenFailed(true));
  };

  return (
    <ActionableNotification
      className="app__notice"
      kind="warning"
      lowContrast
      hideCloseButton
      inline
      title="Full Disk Access is off"
      subtitle={
        openFailed
          ? 'Could not open System Settings. Open Privacy & Security > Full Disk Access manually.'
          : 'Some folders will show as not accessible. Allow this app in System Settings, then restart it.'
      }
      actionButtonLabel="Open System Settings"
      onActionButtonClick={openSettings}
    />
  );
}
