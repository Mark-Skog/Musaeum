import { afterEach, describe, expect, it, vi } from 'vitest';

import { neverWritten, packageLoader } from './package-loader';

const MANIFEST = {
  documents: [{ parts: [{ href: 'text/0000.xhtml', bytes: 120 }] }],
  resources: [{ href: 'styles/0000.css', bytes: 40 }],
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('packageLoader', () => {
  it('knows which files the normalizer never writes', () => {
    expect(neverWritten('META-INF/container.xml')).toBe(false);
    expect(neverWritten('META-INF/encryption.xml')).toBe(true);
    expect(neverWritten('META-INF/com.apple.ibooks.display-options.xml')).toBe(true);
    expect(neverWritten('package.opf')).toBe(false);
    expect(neverWritten('text/META-INF/x.xhtml')).toBe(false);
  });

  it('does not ask the book origin for them, and asks for everything else', async () => {
    const asked: string[] = [];
    vi.stubGlobal('fetch', async (url: string) => {
      asked.push(url);
      if (url === 'pkg/musaeum.json') {
        return new Response(JSON.stringify(MANIFEST));
      }
      if (url === 'pkg/META-INF/container.xml') {
        return new Response('<container/>');
      }
      return new Response('', { status: 404 });
    });
    const loader = await packageLoader();
    expect(await loader.loadText('META-INF/encryption.xml')).toBeNull();
    expect(await loader.loadBlob('META-INF/com.kobobooks.display-options.xml')).toBeNull();
    expect(await loader.loadText('META-INF/container.xml')).toBe('<container/>');
    expect(await loader.loadText('text/missing.xhtml')).toBeNull();
    expect(asked).toEqual(['pkg/musaeum.json', 'pkg/META-INF/container.xml', 'pkg/text/missing.xhtml']);
    expect(loader.getSize('styles/0000.css')).toBe(40);
  });
});
