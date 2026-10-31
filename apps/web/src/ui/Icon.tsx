import type { SVGProps } from 'react';

export type IconName = 'shardrail' | 'sessions' | 'workspace' | 'connection' | 'reconnect' | 'offline' | 'warning' | 'error' | 'verified' | 'not-verified' | 'failed' | 'pending' | 'deny' | 'confirm' | 'expand' | 'collapse' | 'external-link' | 'copy' | 'interrupt' | 'send' | 'close' | 'back' | 'checklist' | 'sun' | 'moon';

export function Icon({ name, ...props }: SVGProps<SVGSVGElement> & { name: IconName }) {
  const paths = {
    shardrail: <><path d="M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4z"/><path d="M16.5 13 21 20h-9z"/></>,
    sessions: <><path d="M4 5h16M4 12h16M4 19h16"/><circle cx="2" cy="5" r=".5"/><circle cx="2" cy="12" r=".5"/><circle cx="2" cy="19" r=".5"/></>,
    workspace: <><path d="M3 7h7l2 2h9v10H3z"/><path d="M3 7V5h7l2 2"/></>,
    connection: <><path d="M5 9a11 11 0 0 1 14 0M8 12a6.5 6.5 0 0 1 8 0M11 15a2 2 0 0 1 2 0"/><circle cx="12" cy="19" r="1"/></>,
    reconnect: <><path d="M20 7v5h-5M4 17v-5h5"/><path d="M5.5 9A7 7 0 0 1 18 7l2 5M4 12l2 5a7 7 0 0 0 12.5-2"/></>,
    offline: <><path d="M3 3 21 21M5 9a11 11 0 0 1 13-1M8 12a6 6 0 0 1 5-1M11 15h2"/><circle cx="12" cy="19" r="1"/></>,
    warning: <><path d="M12 3 2.8 20h18.4z"/><path d="M12 9v5m0 3h.01"/></>,
    error: <><circle cx="12" cy="12" r="9"/><path d="m9 9 6 6m0-6-6 6"/></>,
    verified: <><circle cx="12" cy="12" r="9"/><path d="m8 12 2.5 2.5L16 9"/></>,
    'not-verified': <><circle cx="12" cy="12" r="9"/><path d="M12 7v6m0 3h.01"/></>,
    failed: <><path d="M5 5h14v14H5z"/><path d="m8 8 8 8m0-8-8 8"/></>,
    pending: <><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></>,
    deny: <><circle cx="12" cy="12" r="9"/><path d="M8 8l8 8m0-8-8 8"/></>,
    confirm: <><path d="m5 12 4 4L19 6"/></>,
    expand: <><path d="m6 9 6 6 6-6"/></>,
    collapse: <><path d="m6 15 6-6 6 6"/></>,
    'external-link': <><path d="M14 4h6v6m0-6-9 9"/><path d="M18 13v6H5V6h6"/></>,
    copy: <><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3"/></>,
    interrupt: <><path d="M7 7h10v10H7z"/></>,
    send: <><path d="m22 2-7 20-4-9-9-4z"/><path d="M22 2 11 13"/></>,
    close: <><path d="m6 6 12 12M18 6 6 18"/></>,
    back: <path d="m14 5-7 7 7 7M7 12h13"/>,
    checklist: <><path d="m4 7 2 2 3-3m2 1h9m-16 7 2 2 3-3m2 1h9"/></>,
    sun: <><circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M4.93 4.93l1.42 1.42m11.3 11.3 1.42 1.42M2 12h2m16 0h2M4.93 19.07l1.42-1.42m11.3-11.3 1.42-1.42"/></>,
    moon: <path d="M20.5 15.5A8.5 8.5 0 0 1 8.5 3.5 8.5 8.5 0 1 0 20.5 15.5Z"/>,
  };
  return <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" {...props}>{paths[name]}</svg>;
}
