import { SideNav, SideNavItems, SideNavLink, SideNavDivider } from '@carbon/react';
import Dashboard from '@carbon/icons-react/es/Dashboard';
import FolderDetails from '@carbon/icons-react/es/FolderDetails';

interface AppSideNavProps {
  /** Whether the side nav is expanded (passed from HeaderContainer) */
  isSideNavExpanded: boolean;
}

export function AppSideNav({ isSideNavExpanded }: AppSideNavProps) {
  return (
    <SideNav
      aria-label="Side navigation"
      expanded={isSideNavExpanded}
      isPersistent={false}
    >
      <SideNavItems>
        <SideNavLink href="#" renderIcon={Dashboard} isActive aria-current="page">
          Treemap
        </SideNavLink>
        <SideNavDivider />
        <SideNavLink href="#" renderIcon={FolderDetails}>
          Recent scans
        </SideNavLink>
      </SideNavItems>
    </SideNav>
  );
}
