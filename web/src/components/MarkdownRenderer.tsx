'use client';

import ReactMarkdown from 'react-markdown';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize from 'rehype-sanitize';
import { defaultSchema, type Schema } from 'hast-util-sanitize';

interface MarkdownRendererProps {
  content: string;
}

// Allowlist schema for curator/upstream markdown (curator_note, body_markdown).
// Restricted tag set: structural/inline tags Modrinth about pages use,
// but EXCLUDES dangerous tags (script, iframe, object, embed, input, video,
// audio, link, meta, form, button, textarea, select, option).
// Attributes: only href/src/alt/title/colspan/rowspan/class allowed on safe tags.
// style stripped (blocks CSS-based UI overlay / clickjacking).
// href/src restricted to https only.
const SANITIZE_SCHEMA: Schema = {
  ...defaultSchema,
  tagNames: [
    'p', 'br', 'hr', 'strong', 'em', 'code', 'pre', 'a',
    'ul', 'ol', 'li', 'blockquote',
    'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
    'table', 'thead', 'tbody', 'tr', 'th', 'td',
    'img', 'span', 'div', 'center', 'del', 'sub', 'sup',
    'details', 'summary',
  ],
  attributes: {
    a: ['href', 'title'],
    img: ['src', 'alt', 'title'],
    th: ['colspan', 'rowspan'],
    td: ['colspan', 'rowspan'],
    '*': ['title', 'class'],
  },
  protocols: {
    href: ['https'],
    src: ['https'],
  },
};

export default function MarkdownRenderer({ content }: MarkdownRendererProps) {
  return (
    <ReactMarkdown
      rehypePlugins={[[rehypeRaw, { passThrough: ['html'] }], [rehypeSanitize, SANITIZE_SCHEMA]]}
    >
      {content}
    </ReactMarkdown>
  );
}

