import type { MouseEvent } from 'react';
import {
  Header,
  HeaderGlobalAction,
  HeaderGlobalBar,
  HeaderMenuButton,
  HeaderMenuItem,
  HeaderName,
  HeaderNavigation,
  SkipToContent,
} from '@carbon/react';
import Asleep from '@carbon/icons-react/es/Asleep';
import Light from '@carbon/icons-react/es/Light';
import { useTheme } from '../state/ThemeContext';
import type { AppView } from '../views';

interface AppHeaderProps {
  /** Controlled by HeaderContainer – whether the side nav is expanded */
  isSideNavExpanded: boolean;
  /** Controlled by HeaderContainer – callback to toggle the side nav */
  onClickSideNavExpand: () => void;
  view: AppView;
  onNavigate: (view: AppView) => void;
}

export function AppHeader({
  isSideNavExpanded,
  onClickSideNavExpand,
  view,
  onNavigate,
}: AppHeaderProps) {
  const { theme, toggleTheme } = useTheme();
  const isDark = theme === 'g100';

  const go = (target: AppView) => (event: MouseEvent) => {
    event.preventDefault();
    onNavigate(target);
  };

  return (
    <Header aria-label="Carbon Disk Treemap">
      <SkipToContent />
      <HeaderMenuButton
        aria-label={isSideNavExpanded ? 'Close menu' : 'Open menu'}
        onClick={onClickSideNavExpand}
        isActive={isSideNavExpanded}
        aria-expanded={isSideNavExpanded}
      />
      <HeaderName href="#" prefix="Carbon">
        Disk Treemap
      </HeaderName>
      <HeaderNavigation aria-label="Main">
        <HeaderMenuItem
          href="#treemap"
          isActive={view === 'treemap'}
          aria-current={view === 'treemap' ? 'page' : undefined}
          onClick={go('treemap')}
        >
          Treemap
        </HeaderMenuItem>
        <HeaderMenuItem
          href="#cleanup"
          isActive={view === 'cleanup'}
          aria-current={view === 'cleanup' ? 'page' : undefined}
          onClick={go('cleanup')}
        >
          Clean up caches
        </HeaderMenuItem>
        <HeaderMenuItem
          href="#duplicates"
          isActive={view === 'duplicates'}
          aria-current={view === 'duplicates' ? 'page' : undefined}
          onClick={go('duplicates')}
        >
          Find duplicates
        </HeaderMenuItem>
      </HeaderNavigation>
      <HeaderGlobalBar>
        <HeaderGlobalAction
          aria-label={isDark ? 'Switch to light theme' : 'Switch to dark theme'}
          onClick={toggleTheme}
          tooltipAlignment="end"
        >
          {isDark ? <Light size={20} /> : <Asleep size={20} />}
        </HeaderGlobalAction>
      </HeaderGlobalBar>
    </Header>
  );
}
