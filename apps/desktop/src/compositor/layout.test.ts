import { describe, expect, it } from 'vitest';

import { GEOMETRY_MAGIC } from '../protocol';
import { parseGeometry } from './layout';

function table(pages: Array<[number, number]>): ArrayBuffer {
  const bytes = new ArrayBuffer(8 + pages.length * 8);
  const view = new DataView(bytes);
  view.setUint32(0, GEOMETRY_MAGIC, true);
  view.setUint32(4, pages.length, true);
  pages.forEach(([width, height], index) => {
    view.setFloat32(8 + index * 8, width, true);
    view.setFloat32(12 + index * 8, height, true);
  });
  return bytes;
}

describe('parseGeometry', () => {
  it('reads the table the host produces', () => {
    expect(Array.from(parseGeometry(table([[612, 792], [842, 595]])))).toEqual([612, 792, 842, 595]);
  });

  it('rejects damaged tables', () => {
    expect(() => parseGeometry(new ArrayBuffer(4))).toThrow(/too short/);
    expect(() => parseGeometry(new ArrayBuffer(8))).toThrow(/magic/);
    expect(() => parseGeometry(table([[1, 1]]).slice(0, 12))).toThrow(/length/);
  });
});
