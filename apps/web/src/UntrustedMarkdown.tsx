import React from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeSanitize from 'rehype-sanitize';

export function isConfirmableAgentLink(href: string) {
  try {
    const url = new URL(href);
    return url.protocol === 'http:' || url.protocol === 'https:';
  } catch {
    return false;
  }
}

export function confirmAgentLink(event: { preventDefault(): void }, href: string | undefined, confirm: (message: string) => boolean, open: (url: string) => void) {
  event.preventDefault();
  if (!href || !isConfirmableAgentLink(href)) return;
  if (confirm(`Open agent-provided link?\n${href}`)) open(href);
}

export function UntrustedMarkdown({ source }: { source: string }) {
  const open = (href: string | undefined) => (event: { preventDefault(): void }) => confirmAgentLink(event, href, (message) => window.confirm(message), (url) => { window.open(url, '_blank', 'noopener,noreferrer'); });
  return <ReactMarkdown rehypePlugins={[rehypeSanitize]} components={{
    img: () => null,
    a: ({ href, children }) => <a href={isConfirmableAgentLink(href ?? '') ? href : undefined} rel="noreferrer" onClick={open(href)} onAuxClick={open(href)} onContextMenu={(event) => event.preventDefault()}>{children}</a>,
  }}>{source}</ReactMarkdown>;
}
