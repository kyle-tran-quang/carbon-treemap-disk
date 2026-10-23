import React, {
  createContext,
  useCallback,
  useContext,
  useLayoutEffect,
  useState,
} from 'react';

type Theme = 'white' | 'g100';

interface ThemeContextValue {
  theme: Theme;
  toggleTheme: () => void;
}

const ThemeContext = createContext<ThemeContextValue>({
  theme: 'white',
  toggleTheme: () => undefined,
});

const STORAGE_KEY = 'carbon-treemap-theme';

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [theme, setTheme] = useState<Theme>(() => {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === 'white' || stored === 'g100') return stored;
    // Respect OS preference on first load
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'g100' : 'white';
  });

  useLayoutEffect(() => {
    const root = document.documentElement;
    root.setAttribute('data-carbon-theme', theme);
    // Carbon defines its tokens under `.cds--<theme>` classes, not under the
    // attribute, so the class is what actually switches the theme.
    root.classList.remove('cds--white', 'cds--g100');
    root.classList.add(`cds--${theme}`);
    localStorage.setItem(STORAGE_KEY, theme);
  }, [theme]);

  const toggleTheme = useCallback(() => {
    setTheme((prev) => (prev === 'white' ? 'g100' : 'white'));
  }, []);

  return (
    <ThemeContext.Provider value={{ theme, toggleTheme }}>
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme(): ThemeContextValue {
  return useContext(ThemeContext);
}
