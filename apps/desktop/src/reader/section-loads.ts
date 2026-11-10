/**
 * How the reader asks foliate-js for a section's document.
 *
 * foliate-js gives every file of the book a blob URL and counts who holds it, and two things in
 * that count break when sections load at the same time, as continuous reading loads them:
 *
 * - Two sections that share a stylesheet, loading at once, both find it not made yet and make it
 *   twice. The second replaces the first in the cache with a count of one, so when one of the two
 *   sections is let go, the other's stylesheet is revoked, and a frame that had not read it yet
 *   draws its section without the book's styles. The real books showed it.
 * - A section asked for again while it is loaded is counted the first time only, and let go every
 *   time.
 *
 * So sections load here one after another, and foliate-js loads a section once while anything
 * holds it, and lets go of it when the last holder does.
 */

export interface LoadsDocument {
  load(): Promise<string>;
  unload(): void;
}

/** How long a load may hold up the ones after it, in case it never ends. */
const PATIENCE_MS = 15_000;

const guarded = new WeakSet<object>();

/** Resolves when `before` is done, or after `PATIENCE_MS`, whichever is first. */
function after(before: Promise<unknown>): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, PATIENCE_MS);
    void before.then(() => {
      clearTimeout(timer);
      resolve();
    });
  });
}

export function guardSectionLoads(sections: readonly (LoadsDocument | null | undefined)[]): void {
  let queue: Promise<unknown> = Promise.resolve();
  for (const section of sections) {
    if (section === null || section === undefined || guarded.has(section)) {
      continue;
    }
    guarded.add(section);
    const load = section.load.bind(section);
    const unload = section.unload.bind(section);
    let holders = 0;
    let url: Promise<string> | null = null;

    section.load = () => {
      holders++;
      if (url === null) {
        const loading = after(queue).then(load);
        queue = loading.catch(() => undefined);
        url = loading;
        // A load that failed leaves nobody holding the section, and the next one tries again.
        loading.catch(() => {
          if (url === loading) {
            url = null;
            holders = 0;
          }
        });
      }
      return url;
    };

    section.unload = () => {
      if (holders === 0) {
        return;
      }
      holders--;
      if (holders > 0 || url === null) {
        return;
      }
      const current = url;
      // Let go once the load is done, unless someone asked for the section again meanwhile.
      current.then(
        () => {
          if (holders === 0 && url === current) {
            url = null;
            unload();
          }
        },
        () => undefined,
      );
    };
  }
}
