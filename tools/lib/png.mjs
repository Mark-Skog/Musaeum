// Reads the PNG pictures Chromium's screenshots are: 8 bits a channel, RGB or RGBA, not interlaced.
import { inflateSync } from 'node:zlib';

/** The picture's pixels as RGBA rows, with its width and height. */
export function decodePng(bytes) {
  const buffer = Buffer.from(bytes);
  if (buffer.readUInt32BE(0) !== 0x89504e47) {
    throw new Error('not a PNG');
  }
  let at = 8;
  let width = 0;
  let height = 0;
  let channels = 0;
  const data = [];
  while (at < buffer.length) {
    const length = buffer.readUInt32BE(at);
    const type = buffer.toString('latin1', at + 4, at + 8);
    const body = buffer.subarray(at + 8, at + 8 + length);
    if (type === 'IHDR') {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      const depth = body[8];
      const color = body[9];
      if (depth !== 8 || (color !== 2 && color !== 6) || body[12] !== 0) {
        throw new Error(`a PNG of depth ${depth}, color type ${color}, interlace ${body[12]} is not read here`);
      }
      channels = color === 6 ? 4 : 3;
    } else if (type === 'IDAT') {
      data.push(body);
    } else if (type === 'IEND') {
      break;
    }
    at += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(data));
  const stride = width * channels;
  const pixels = Buffer.alloc(width * height * 4);
  const previous = Buffer.alloc(stride);
  const line = Buffer.alloc(stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const row = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? line[x - channels] : 0;
      const up = previous[x];
      const corner = x >= channels ? previous[x - channels] : 0;
      let value = row[x];
      if (filter === 1) value += left;
      else if (filter === 2) value += up;
      else if (filter === 3) value += (left + up) >> 1;
      else if (filter === 4) {
        const p = left + up - corner;
        const pa = Math.abs(p - left);
        const pb = Math.abs(p - up);
        const pc = Math.abs(p - corner);
        value += pa <= pb && pa <= pc ? left : pb <= pc ? up : corner;
      }
      line[x] = value & 255;
    }
    for (let x = 0; x < width; x++) {
      for (let c = 0; c < 4; c++) {
        pixels[(y * width + x) * 4 + c] = c < channels ? line[x * channels + c] : 255;
      }
    }
    line.copy(previous);
  }
  return { width, height, pixels };
}
