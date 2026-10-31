import type { ReactNode } from 'react';

export function StatusPill({ children, tone = 'neutral' }: { children: ReactNode; tone?: 'neutral' | 'warning' | 'danger' | 'success' }) {
  return <span className={`status-pill status-${tone}`}>{children}</span>;
}
