import type { MouseEvent } from 'react';
import { SideNav, SideNavItems, SideNavLink, SideNavDivider } from '@carbon/react';
import Copy from '@carbon/icons-react/es/Copy';
import Dashboard from '@carbon/icons-react/es/Dashboard';
import FolderDetails from '@carbon/icons-react/es/FolderDetails';
import TrashCan from '@carbon/icons-react/es/TrashCan';
import type { AppView } from '../views';

interface AppSideNavProps {
  /** Whether the side nav is expanded (passed from HeaderContainer) */
  isSideNavExpanded: boolean;
  view: AppView;
  onNavigate: (view: AppView) => void;
}

export function AppSideNav({ isSideNavExpanded, view, onNavigate }: AppSideNavProps) {
  const go = (target: AppView) => (event: MouseEvent) => {
    event.preventDefault();
    onNavigate(target);
  };

  return (
    <SideNav
      aria-label="Side navigation"
      expanded={isSideNavExpanded}
      isPersistent={false}
    >
      <SideNavItems>
        <SideNavLink
          href="#treemap"
          renderIcon={Dashboard}
          isActive={view === 'treemap'}
          aria-current={view === 'treemap' ? 'page' : undefined}
          onClick={go('treemap')}
        >
          Treemap
        </SideNavLink>
        <SideNavLink
          href="#cleanup"
          renderIcon={TrashCan}
          isActive={view === 'cleanup'}
          aria-current={view === 'cleanup' ? 'page' : undefined}
          onClick={go('cleanup')}
        >
          Clean up caches
        </SideNavLink>
        <SideNavLink
          href="#duplicates"
          renderIcon={Copy}
          isActive={view === 'duplicates'}
          aria-current={view === 'duplicates' ? 'page' : undefined}
          onClick={go('duplicates')}
        >
          Find duplicates
        </SideNavLink>
        <SideNavDivider />
        <SideNavLink href="#" renderIcon={FolderDetails}>
          Recent scans
        </SideNavLink>
      </SideNavItems>
    </SideNav>
  );
}
