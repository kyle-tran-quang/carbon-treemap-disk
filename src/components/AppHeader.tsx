import {
  Header,
  HeaderGlobalAction,
  HeaderGlobalBar,
  HeaderMenuButton,
  HeaderName,
  SkipToContent,
} from '@carbon/react';
import Asleep from '@carbon/icons-react/es/Asleep';
import Light from '@carbon/icons-react/es/Light';
import { useTheme } from '../state/ThemeContext';

interface AppHeaderProps {
  /** Controlled by HeaderContainer – whether the side nav is expanded */
  isSideNavExpanded: boolean;
  /** Controlled by HeaderContainer – callback to toggle the side nav */
  onClickSideNavExpand: () => void;
}

export function AppHeader({ isSideNavExpanded, onClickSideNavExpand }: AppHeaderProps) {
  const { theme, toggleTheme } = useTheme();
  const isDark = theme === 'g100';

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
