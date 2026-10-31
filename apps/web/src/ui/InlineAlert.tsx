import type { ReactNode } from 'react';

export function InlineAlert({ children, role = 'alert' }: { children: ReactNode; role?: 'alert' | 'status' | 'note' }) {
  return <p className="inline-alert" role={role}>{children}</p>;
}
