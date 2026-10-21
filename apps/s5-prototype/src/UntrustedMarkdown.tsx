import React from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeSanitize from 'rehype-sanitize';

export function UntrustedMarkdown({ source }: { source: string }) {
  return <ReactMarkdown rehypePlugins={[rehypeSanitize]} components={{
    img: () => null,
    a: ({ href, children }) => <a href={href} target="_blank" rel="noreferrer" onClick={(event) => {
      event.preventDefault();
      if (href && window.confirm(`Open agent-provided link?\n${href}`)) window.open(href, '_blank', 'noopener');
    }}>{children}</a>,
  }}>{source}</ReactMarkdown>;
}
