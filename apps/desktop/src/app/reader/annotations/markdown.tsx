/**
 * Notes are Markdown, shown with a small renderer of our own: paragraphs, headings, lists, quotes,
 * code, emphasis and links. It makes React elements and never HTML, so whatever a note says is
 * text: a tag in a note is shown as typed. A link shows its address and, when the page gives a
 * handler, a click on it goes to the same dialog as a link of a book (copy, or open in the browser
 * for http and https); a link of any other scheme is only text.
 */

import type { ReactNode } from 'react';

/** The schemes a link in a note may have to be offered to the link dialog, as a book's links are. */
const LINKABLE = /^(https?:|mailto:|tel:)/i;

export type LinkHandler = (href: string) => void;

export type Block =
  | { type: 'heading'; level: number; text: string }
  | { type: 'paragraph'; text: string }
  | { type: 'list'; ordered: boolean; items: string[] }
  | { type: 'quote'; text: string }
  | { type: 'code'; text: string };

/** The blocks of a note. */
export function parseBlocks(source: string): Block[] {
  const blocks: Block[] = [];
  const lines = source.replace(/\r\n?/g, '\n').split('\n');
  let at = 0;
  while (at < lines.length) {
    const line = lines[at];
    if (line.trim() === '') {
      at++;
      continue;
    }
    if (line.startsWith('```')) {
      const code: string[] = [];
      at++;
      while (at < lines.length && !lines[at].startsWith('```')) {
        code.push(lines[at]);
        at++;
      }
      at++;
      blocks.push({ type: 'code', text: code.join('\n') });
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading !== null) {
      blocks.push({ type: 'heading', level: heading[1].length, text: heading[2].trim() });
      at++;
      continue;
    }
    if (/^\s*([-*+]|\d+[.)])\s+/.test(line)) {
      const ordered = /^\s*\d+[.)]\s+/.test(line);
      const items: string[] = [];
      while (at < lines.length && /^\s*([-*+]|\d+[.)])\s+/.test(lines[at])) {
        items.push(lines[at].replace(/^\s*([-*+]|\d+[.)])\s+/, ''));
        at++;
      }
      blocks.push({ type: 'list', ordered, items });
      continue;
    }
    if (line.startsWith('>')) {
      const quoted: string[] = [];
      while (at < lines.length && lines[at].startsWith('>')) {
        quoted.push(lines[at].replace(/^>\s?/, ''));
        at++;
      }
      blocks.push({ type: 'quote', text: quoted.join(' ') });
      continue;
    }
    const paragraph: string[] = [];
    while (
      at < lines.length &&
      lines[at].trim() !== '' &&
      !/^(#{1,6}\s|```|>|\s*([-*+]|\d+[.)])\s+)/.test(lines[at])
    ) {
      paragraph.push(lines[at].trim());
      at++;
    }
    blocks.push({ type: 'paragraph', text: paragraph.join(' ') });
  }
  return blocks;
}

/** Emphasis, code and links inside a block, as React nodes. */
export function inline(text: string, key = 0, onLink?: LinkHandler): ReactNode[] {
  const nodes: ReactNode[] = [];
  const pattern = /(`[^`]+`)|(\*\*[^*]+\*\*|__[^_]+__)|(\*[^*\s][^*]*\*|_[^_\s][^_]*_)|(\[[^\]]+\]\([^)\s]+\))/;
  let rest = text;
  let index = key;
  while (rest.length > 0) {
    const found = pattern.exec(rest);
    if (found === null) {
      nodes.push(rest);
      break;
    }
    if (found.index > 0) {
      nodes.push(rest.slice(0, found.index));
    }
    const token = found[0];
    if (found[1] !== undefined) {
      nodes.push(<code key={index++}>{token.slice(1, -1)}</code>);
    } else if (found[2] !== undefined) {
      nodes.push(<strong key={index++}>{inline(token.slice(2, -2), index * 100, onLink)}</strong>);
    } else if (found[3] !== undefined) {
      nodes.push(<em key={index++}>{inline(token.slice(1, -1), index * 100, onLink)}</em>);
    } else {
      const link = /^\[([^\]]+)\]\(([^)\s]+)\)$/.exec(token);
      const href = link?.[2] ?? '';
      const label = link?.[1] ?? token;
      if (onLink !== undefined && LINKABLE.test(href)) {
        nodes.push(
          <button key={index++} type="button" className="note-link" title={href} onClick={() => onLink(href)}>
            {label} <span className="note-link-target">({href})</span>
          </button>,
        );
      } else {
        nodes.push(
          <span key={index++} className="note-link" title={href}>
            {label} <span className="note-link-target">({href})</span>
          </span>,
        );
      }
    }
    rest = rest.slice(found.index + token.length);
  }
  return nodes;
}

/** A note, rendered. `onLink` is what a click on a web link does; without it, links are text. */
export function Markdown({ source, onLink }: { source: string; onLink?: LinkHandler }): React.JSX.Element {
  return (
    <div className="markdown">
      {parseBlocks(source).map((block, index) => {
        switch (block.type) {
          case 'heading': {
            const Tag = `h${Math.min(6, block.level + 2)}` as 'h3';
            return <Tag key={index}>{inline(block.text, 0, onLink)}</Tag>;
          }
          case 'list': {
            const items = block.items.map((item, at) => <li key={at}>{inline(item, 0, onLink)}</li>);
            return block.ordered ? <ol key={index}>{items}</ol> : <ul key={index}>{items}</ul>;
          }
          case 'quote':
            return <blockquote key={index}>{inline(block.text, 0, onLink)}</blockquote>;
          case 'code':
            return (
              <pre key={index}>
                <code>{block.text}</code>
              </pre>
            );
          default:
            return <p key={index}>{inline(block.text, 0, onLink)}</p>;
        }
      })}
    </div>
  );
}
