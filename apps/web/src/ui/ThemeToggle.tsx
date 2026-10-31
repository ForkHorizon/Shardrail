import { useLayoutEffect, useState } from 'react';
import { ToggleButton, Tooltip, TooltipTrigger } from 'react-aria-components';
import { Icon } from './Icon';

const key = 'shardrail-theme';
type Theme = 'light' | 'dark';
function systemTheme(): Theme { return matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'; }
function storedTheme(): Theme | null { const value = localStorage.getItem(key); return value === 'light' || value === 'dark' ? value : null; }

export function ThemeToggle() {
  const [theme, setTheme] = useState<Theme>(() => storedTheme() ?? systemTheme());
  useLayoutEffect(() => { const saved = storedTheme(); if (saved) document.documentElement.dataset.theme = saved; }, []);
  function toggle() {
    const next = theme === 'dark' ? 'light' : 'dark';
    document.documentElement.dataset.theme = next;
    localStorage.setItem(key, next);
    setTheme(next);
  }
  const label = `Use ${theme === 'dark' ? 'light' : 'dark'} theme`;
  return <TooltipTrigger>
    <ToggleButton className="button-quiet theme-toggle" isSelected={theme === 'dark'} onChange={toggle} aria-label={label}><Icon className="icon" name={theme === 'dark' ? 'sun' : 'moon'}/></ToggleButton>
    <Tooltip>{label}</Tooltip>
  </TooltipTrigger>;
}
