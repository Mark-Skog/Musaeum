/** Line models as the host sends them, for the compositor's tests. */

import { toStored } from './chars';
import { LINE_TURNED, LINES_MAGIC, LINES_VERSION } from './lines';

export interface Line {
  column?: number;
  /** As shown. */
  points: [number, number][];
  /** Fractions of the baseline's length. */
  gaps?: [number, number][];
  ascent?: number;
  descent?: number;
  turned?: boolean;
}

/** Encodes lines as the host sends them: the page hash, then `MLIN` with points as stored. */
export function encodeLines(lines: Line[], { rotation = 0, columns = 1, hash = [0x89abcdef, 0x01234567], version = LINES_VERSION } = {}): ArrayBuffer {
  const size = 8 + 24 + columns * 16 + lines.reduce((sum, line) => sum + 20 + (line.points.length + (line.gaps?.length ?? 0)) * 8, 0);
  const buffer = new ArrayBuffer(size);
  const view = new DataView(buffer);
  view.setUint32(0, hash[0], true);
  view.setUint32(4, hash[1], true);
  let at = 8;
  view.setUint32(at, LINES_MAGIC, true);
  view.setUint32(at + 4, version, true);
  view.setUint8(at + 8, rotation);
  view.setFloat32(at + 12, 0, true);
  view.setUint32(at + 16, columns, true);
  view.setUint32(at + 20, lines.length, true);
  at += 24;
  for (let column = 0; column < columns; column++) {
    [0, 0, 1, 1].forEach((value) => {
      view.setFloat32(at, value, true);
      at += 4;
    });
  }
  for (const line of lines) {
    const gaps = line.gaps ?? [];
    view.setUint16(at, line.column ?? 0, true);
    view.setUint16(at + 2, line.points.length, true);
    view.setUint16(at + 4, gaps.length, true);
    view.setUint16(at + 6, line.turned === true ? LINE_TURNED : 0, true);
    view.setFloat32(at + 8, 0.01, true);
    view.setFloat32(at + 12, line.ascent ?? 0.015, true);
    view.setFloat32(at + 16, line.descent ?? 0.004, true);
    at += 20;
    for (const [x, y] of line.points) {
      const [u, v] = toStored(x, y, rotation);
      view.setFloat32(at, u, true);
      view.setFloat32(at + 4, v, true);
      at += 8;
    }
    for (const [from, to] of gaps) {
      view.setFloat32(at, from, true);
      view.setFloat32(at + 4, to, true);
      at += 8;
    }
  }
  return buffer;
}
