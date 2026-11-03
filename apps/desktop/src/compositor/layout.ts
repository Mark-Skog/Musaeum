/** The host's page-size table. */

import { GEOMETRY_MAGIC } from '../protocol';

/** Decodes the host's page-size table into `[width, height]` pairs, in points. */
export function parseGeometry(bytes: ArrayBuffer): Float32Array {
  if (bytes.byteLength < 8) {
    throw new Error('geometry table is too short');
  }
  const header = new DataView(bytes);
  if (header.getUint32(0, true) !== GEOMETRY_MAGIC) {
    throw new Error('geometry table has a bad magic number');
  }
  const count = header.getUint32(4, true);
  if (bytes.byteLength !== 8 + count * 8) {
    throw new Error('geometry table length does not match its page count');
  }
  return new Float32Array(bytes, 8, count * 2);
}
