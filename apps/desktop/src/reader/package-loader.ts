/**
 * Reads a normalized book file by file from the book's own origin, for foliate-js's EPUB class.
 *
 * The package is already an EPUB 3 with every file in place, so nothing here unzips anything; and
 * foliate-js asks for sizes synchronously, which the manifest written by the normalizer has.
 */

import type { EpubLoader } from '../../../../vendor/foliate-js/epub.js';

interface ManifestSizes {
  documents: { parts: { href: string; bytes: number }[] }[];
  resources: { href: string; bytes: number }[];
}

/** A path in the package, as the protocol takes it. */
function packageUrl(path: string): string {
  return `pkg/${path.split('/').map(encodeURIComponent).join('/')}`;
}

/**
 * Whether `path` is a file the normalizer never writes. It writes one file in `META-INF`, and
 * foliate-js asks for others a book may carry (encryption, display options), each of which would be
 * a failed request in the console of every book opened.
 */
export function neverWritten(path: string): boolean {
  return path.startsWith('META-INF/') && path !== 'META-INF/container.xml';
}

async function fetchOk(path: string): Promise<Response | null> {
  if (neverWritten(path)) {
    return null;
  }
  const response = await fetch(packageUrl(path), { cache: 'no-store', credentials: 'omit' });
  return response.ok ? response : null;
}

export async function packageLoader(): Promise<EpubLoader> {
  const manifest = (await (await fetchOk('musaeum.json'))?.json()) as ManifestSizes | undefined;
  if (manifest === undefined) {
    throw new Error('the book has no manifest');
  }
  const sizes = new Map<string, number>();
  for (const document of manifest.documents) {
    for (const part of document.parts) {
      sizes.set(part.href, part.bytes);
    }
  }
  for (const resource of manifest.resources) {
    sizes.set(resource.href, resource.bytes);
  }
  return {
    loadText: async (name) => (await (await fetchOk(name))?.text()) ?? null,
    loadBlob: async (name, type) => {
      const response = await fetchOk(name);
      if (response === null) {
        return null;
      }
      const bytes = await response.arrayBuffer();
      return new Blob([bytes], { type: type ?? response.headers.get('content-type') ?? '' });
    },
    getSize: (name) => sizes.get(name) ?? 0,
  };
}
