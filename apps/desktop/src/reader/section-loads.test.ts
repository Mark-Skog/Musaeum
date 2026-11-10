import { describe, expect, it } from 'vitest';

import { guardSectionLoads } from './section-loads';

/** A section whose loads finish when the test says, counting what foliate-js would be asked. */
function fakeSection(name: string, log: string[]) {
  const pending: { resolve: (url: string) => void; reject: (error: Error) => void }[] = [];
  const section = {
    loads: 0,
    unloads: 0,
    load(): Promise<string> {
      section.loads++;
      log.push(`start ${name}`);
      return new Promise<string>((resolve, reject) => {
        pending.push({
          resolve: (url) => {
            log.push(`end ${name}`);
            resolve(url);
          },
          reject,
        });
      });
    },
    unload(): void {
      section.unloads++;
      log.push(`unload ${name}`);
    },
    finish(url = `blob:${name}`): void {
      pending.shift()?.resolve(url);
    },
    fail(): void {
      pending.shift()?.reject(new Error(`${name} failed`));
    },
  };
  return section;
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('guardSectionLoads', () => {
  it('loads one section after another, never two at once', async () => {
    const log: string[] = [];
    const a = fakeSection('a', log);
    const b = fakeSection('b', log);
    guardSectionLoads([a, b]);
    const first = a.load();
    const second = b.load();
    await settle();
    expect(log).toEqual(['start a']);
    a.finish();
    await expect(first).resolves.toBe('blob:a');
    await settle();
    expect(log).toEqual(['start a', 'end a', 'start b']);
    b.finish();
    await expect(second).resolves.toBe('blob:b');
  });

  it('loads a section once while anything holds it, and lets it go with the last holder', async () => {
    const log: string[] = [];
    const a = fakeSection('a', log);
    guardSectionLoads([a]);
    const first = a.load();
    const second = a.load();
    await settle();
    a.finish();
    expect(await first).toBe('blob:a');
    expect(await second).toBe('blob:a');
    expect(a.loads).toBe(1);
    a.unload();
    await settle();
    expect(a.unloads).toBe(0);
    a.unload();
    await settle();
    expect(a.unloads).toBe(1);
    // One more than it was asked for is not passed on.
    a.unload();
    await settle();
    expect(a.unloads).toBe(1);
  });

  it('lets go of a section still loading once the load is done', async () => {
    const log: string[] = [];
    const a = fakeSection('a', log);
    guardSectionLoads([a]);
    const loading = a.load();
    a.unload();
    await settle();
    expect(a.unloads).toBe(0);
    a.finish();
    await loading;
    await settle();
    expect(log).toEqual(['start a', 'end a', 'unload a']);
  });

  it('keeps a section asked for again before it was let go', async () => {
    const log: string[] = [];
    const a = fakeSection('a', log);
    guardSectionLoads([a]);
    const loading = a.load();
    a.unload();
    const again = a.load();
    await settle();
    a.finish();
    expect(await loading).toBe('blob:a');
    expect(await again).toBe('blob:a');
    await settle();
    expect(a.loads).toBe(1);
    expect(a.unloads).toBe(0);
  });

  it('loads a section again after it was let go', async () => {
    const log: string[] = [];
    const a = fakeSection('a', log);
    guardSectionLoads([a]);
    const first = a.load();
    await settle();
    a.finish('blob:a1');
    await first;
    a.unload();
    await settle();
    const second = a.load();
    await settle();
    a.finish('blob:a2');
    expect(await second).toBe('blob:a2');
    expect(a.loads).toBe(2);
    expect(a.unloads).toBe(1);
  });

  it('tries again after a load that failed, which nobody holds', async () => {
    const log: string[] = [];
    const a = fakeSection('a', log);
    const b = fakeSection('b', log);
    guardSectionLoads([a, b]);
    const failing = a.load();
    const next = b.load();
    await settle();
    a.fail();
    await expect(failing).rejects.toThrow('a failed');
    // The failure does not hold up the sections after it.
    await settle();
    b.finish();
    await expect(next).resolves.toBe('blob:b');
    a.unload();
    await settle();
    expect(a.unloads).toBe(0);
    const retry = a.load();
    await settle();
    a.finish();
    await expect(retry).resolves.toBe('blob:a');
    expect(a.loads).toBe(2);
  });

  it('wraps a section once, however often it is asked to', async () => {
    const log: string[] = [];
    const a = fakeSection('a', log);
    guardSectionLoads([a, null]);
    guardSectionLoads([a]);
    const loading = a.load();
    await settle();
    a.finish();
    await loading;
    a.unload();
    await settle();
    expect(a.loads).toBe(1);
    expect(a.unloads).toBe(1);
  });
});
