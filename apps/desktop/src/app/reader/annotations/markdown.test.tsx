import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { Markdown, parseBlocks } from './markdown';

const html = (source: string): string => renderToStaticMarkup(<Markdown source={source} />);

describe('notes in Markdown', () => {
  it('has blocks: headings, paragraphs, lists, quotes and code', () => {
    const blocks = parseBlocks('# Título\n\nUma linha\ne outra.\n\n- um\n- dois\n\n1. primeiro\n\n> citado\n\n```\ncódigo <b>\n```');
    expect(blocks.map((block) => block.type)).toEqual(['heading', 'paragraph', 'list', 'list', 'quote', 'code']);
    expect(blocks[1]).toEqual({ type: 'paragraph', text: 'Uma linha e outra.' });
  });

  it('renders emphasis, code and links as elements and never as HTML', () => {
    expect(html('**forte** e *leve* com `x<y`')).toBe(
      '<div class="markdown"><p><strong>forte</strong> e <em>leve</em> com <code>x&lt;y</code></p></div>',
    );
    const hostile = html('<img src=x onerror=alert(1)> [clique](javascript:alert(1))');
    expect(hostile).not.toContain('<img');
    expect(hostile).not.toContain('href');
    expect(hostile).toContain('&lt;img src=x onerror=alert(1)&gt;');
    expect(hostile).toContain('javascript:alert(1');
  });

  it("offers a web link to the page's link dialog, and any other scheme only as text", () => {
    const opened: string[] = [];
    const page = renderToStaticMarkup(
      <Markdown
        source="Veja [o site](https://example.org/x) e [isto](javascript:alert(1)) e [e-mail](mailto:a@b.c)"
        onLink={(href) => opened.push(href)}
      />,
    );
    expect(page.match(/<button/g)).toHaveLength(2);
    expect(page).toContain('title="https://example.org/x"');
    expect(page).not.toContain('<a ');
    // Without a handler, every link is text.
    expect(renderToStaticMarkup(<Markdown source="[o site](https://example.org/x)" />)).not.toContain('<button');
  });
});
