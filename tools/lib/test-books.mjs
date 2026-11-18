// Small books written for the annotation tests, each in two revisions: a text PDF and an EPUB whose
// second edition has something added before the marked text, a sentence edited and a passage gone.
// Nothing here needs a library: PDF and ZIP are written by hand, with stored (uncompressed) entries;
// only the pictures of a scanned book are compressed, by node:zlib.

import { deflateSync } from 'node:zlib';

/**
 * A PDF of text pages in Helvetica, one string per line, WinAnsi-encoded so accents come through.
 * A page listed in `dark` is white type on a black ground, as a scanned slide or an inverted page.
 */
export function textPdf(pages, { dark = [] } = {}) {
  const objects = [];
  const add = (body) => {
    objects.push(body);
    return objects.length;
  };
  const catalog = add(null);
  const pagesNode = add(null);
  const font = add('<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>');
  const kids = [];
  pages.forEach((lines, number) => {
    const escape = (line) => line.replace(/\\/g, '\\\\').replace(/\(/g, '\\(').replace(/\)/g, '\\)');
    const text = lines.map((line, index) => `${index === 0 ? '' : '0 -18 Td '}(${escape(line)}) Tj`).join('\n');
    const ground = dark.includes(number) ? '0 0 0 rg 0 0 612 792 re f 1 1 1 rg\n' : '';
    const stream = `${ground}BT /F1 12 Tf 72 740 Td\n${text}\nET`;
    const content = add(`<< /Length ${Buffer.byteLength(stream, 'latin1')} >>\nstream\n${stream}\nendstream`);
    kids.push(add(`<< /Type /Page /Parent ${pagesNode} 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 ${font} 0 R >> >> /Contents ${content} 0 R >>`));
  });
  objects[catalog - 1] = `<< /Type /Catalog /Pages ${pagesNode} 0 R >>`;
  objects[pagesNode - 1] = `<< /Type /Pages /Kids [${kids.map((kid) => `${kid} 0 R`).join(' ')}] /Count ${kids.length} >>`;
  let out = '%PDF-1.7\n';
  const offsets = [];
  objects.forEach((object, index) => {
    offsets.push(Buffer.byteLength(out, 'latin1'));
    out += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const xref = Buffer.byteLength(out, 'latin1');
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) {
    out += `${String(offset).padStart(10, '0')} 00000 n \n`;
  }
  out += `trailer\n<< /Size ${objects.length + 1} /Root ${catalog} 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(out, 'latin1');
}

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/** A ZIP with every entry stored, in order. */
export function storedZip(files) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const { name, data } of files) {
    const nameBytes = Buffer.from(name, 'utf8');
    const body = Buffer.isBuffer(data) ? data : Buffer.from(data, 'utf8');
    const crc = crc32(body);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x0800, 6);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(body.length, 18);
    local.writeUInt32LE(body.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    locals.push(local, nameBytes, body);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(body.length, 20);
    central.writeUInt32LE(body.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt32LE(offset, 42);
    centrals.push(central, nameBytes);
    offset += 30 + nameBytes.length + body.length;
  }
  const directory = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(files.length, 8);
  end.writeUInt16LE(files.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}

const xml = (text) => text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/** An EPUB 3 of chapters, each a title and paragraphs, by `creators` if given. */
export function epub({ title, identifier, chapters, creators = [] }) {
  const items = chapters.map((_, index) => `<item id="c${index + 1}" href="c${index + 1}.xhtml" media-type="application/xhtml+xml"/>`).join('');
  const spine = chapters.map((_, index) => `<itemref idref="c${index + 1}"/>`).join('');
  const nav = chapters.map((chapter, index) => `<li><a href="c${index + 1}.xhtml">${xml(chapter.title)}</a></li>`).join('');
  const files = [
    { name: 'mimetype', data: 'application/epub+zip' },
    {
      name: 'META-INF/container.xml',
      data: '<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>',
    },
    {
      name: 'OEBPS/content.opf',
      data: `<?xml version="1.0" encoding="utf-8"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">${xml(identifier)}</dc:identifier><dc:title>${xml(title)}</dc:title>${creators.map((name) => `<dc:creator>${xml(name)}</dc:creator>`).join('')}<dc:language>pt</dc:language><meta property="dcterms:modified">2026-11-10T00:00:00Z</meta></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>${items}</manifest><spine>${spine}</spine></package>`,
    },
    {
      name: 'OEBPS/nav.xhtml',
      data: `<?xml version="1.0" encoding="utf-8"?><!DOCTYPE html><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Sumário</title></head><body><nav epub:type="toc"><ol>${nav}</ol></nav></body></html>`,
    },
    ...chapters.map((chapter, index) => ({
      name: `OEBPS/c${index + 1}.xhtml`,
      data: `<?xml version="1.0" encoding="utf-8"?><!DOCTYPE html><html xmlns="http://www.w3.org/1999/xhtml"><head><title>${xml(chapter.title)}</title></head><body><h1>${xml(chapter.title)}</h1>\n${chapter.paragraphs.map((paragraph) => `<p>${xml(paragraph)}</p>`).join('\n')}\n</body></html>`,
    })),
  ];
  return storedZip(files);
}

const WORDS = [
  'leitor', 'página', 'memória', 'janela', 'rio', 'cidade', 'carta', 'jardim', 'pedra', 'vento', 'livro',
  'promessa', 'silêncio', 'estrada', 'luz', 'manhã', 'tempo', 'casa', 'mar', 'noite', 'voz', 'caminho',
];

/** A sentence that appears nowhere else in the book: its number spelled out, and words in turn. */
export function sentence(tag, seed, words = 9) {
  const picked = [];
  let state = seed * 2654435761 % 4294967296;
  for (let at = 0; at < words; at++) {
    state = (state * 1103515245 + 12345) % 2147483648;
    picked.push(WORDS[state % WORDS.length]);
  }
  return `${tag}: ${picked.join(' ')}.`;
}

/** The PDF in its two revisions, and the lines the test marks. */
export function annotatedPdf() {
  const page = (number, lines) => Array.from({ length: lines }, (_, line) => sentence(`Página ${number}, linha ${line + 1}`, number * 100 + line));
  // Full pages: the last line of one and the first of the next fit on the screen together. The
  // fourth is white type on black, where a highlight has to show the other way round.
  const v1 = [page(1, 36), page(2, 36), page(3, 36), page(4, 20)];
  // What the second edition edits and removes reads like nothing else in the book.
  v1[1][3] = 'Uma linha que a segunda edição muda: o farol acende às seis da tarde.';
  v1[2][5] = 'Esta linha some na segunda edição: zebra, quimera e xilogravura.';
  const targets = {
    kept: v1[0][1],
    word: 'inesquecível',
    edited: v1[1][3],
    across: [v1[0][35], v1[1][0]],
    removed: v1[2][5],
    dark: v1[3][2],
  };
  v1[0][4] = `Uma linha com uma palavra inesquecível no meio dela.`;
  const v2 = v1.map((lines) => lines.slice());
  v2[1][3] = v2[1][3].replace('o farol', 'o moinho');
  v2[2].splice(5, 1);
  v2.unshift(['Prefácio desta edição', 'Uma página nova antes de tudo o que havia.']);
  return { v1: textPdf(v1, { dark: [3] }), v2: textPdf(v2, { dark: [4] }), targets, editedTo: v2[2][3] };
}

/** The EPUB in its two editions, with a chapter long enough to be cut, and what the test marks. */
export function annotatedEpub() {
  const chapter = (number, count) => Array.from({ length: count }, (_, at) => sentence(`Capítulo ${number}, parágrafo ${at + 1}`, number * 10000 + at, 24));
  const chapters = [
    { title: 'Primeiro', paragraphs: chapter(1, 20) },
    { title: 'Segundo, longo', paragraphs: chapter(2, 1400) },
    { title: 'Terceiro', paragraphs: chapter(3, 20) },
  ];
  chapters[0].paragraphs[9] =
    'Este parágrafo some na segunda edição: a zebra, a quimera e a xilogravura dormem juntas no sótão da casa velha, longe de tudo.';
  chapters[2].paragraphs[4] =
    'Um parágrafo que a segunda edição muda: o farol da baía acende às seis horas e ilumina os barcos que voltam devagar ao porto.';
  const targets = {
    kept: chapters[0].paragraphs[2],
    edited: chapters[2].paragraphs[4],
    removed: chapters[0].paragraphs[9],
  };
  const v1 = epub({ title: 'Livro para anotar', identifier: 'urn:musaeum:test:anotar', chapters });
  const revised = chapters.map((entry) => ({ ...entry, paragraphs: entry.paragraphs.slice() }));
  revised[0].paragraphs.splice(9, 1);
  revised[0].paragraphs.unshift('Um parágrafo novo, posto antes de tudo nesta segunda edição do livro.');
  revised[2].paragraphs[4] = revised[2].paragraphs[4].replace('o farol', 'o moinho');
  const v2 = epub({ title: 'Livro para anotar', identifier: 'urn:musaeum:test:anotar', chapters: revised });
  return { v1, v2, targets };
}

/**
 * A scanned book: pages that are only a picture of letters, with no text layer, as a scan without
 * OCR is. Returns the PDF and where its lines are: for each page, each line's baseline as a fraction
 * of the page's height and its words as fractions of its width.
 */
export function scanPdf(pageCount = 2) {
  const width = 1224;
  const height = 1584;
  const xHeight = 18;
  const pitch = 44;
  const stroke = 3;
  let seed = 7;
  const random = () => {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    return seed / 0x7fffffff;
  };
  const pages = [];
  const images = [];
  for (let page = 0; page < pageCount; page++) {
    const pixels = new Uint8Array(width * height);
    for (let at = 0; at < pixels.length; at++) {
      pixels[at] = 228 + Math.floor(random() * 10);
    }
    const ink = (x0, y0, x1, y1) => {
      for (let y = Math.max(0, Math.floor(y0)); y < Math.min(height, Math.ceil(y1)); y++) {
        for (let x = Math.max(0, Math.floor(x0)); x < Math.min(width, Math.ceil(x1)); x++) {
          pixels[y * width + x] = 30 + Math.floor(random() * 30);
        }
      }
    };
    const lines = [];
    for (let baseline = 170; baseline + pitch < height - 120; baseline += pitch) {
      const words = [];
      let x = 120;
      while (x < width - 220) {
        const letters = 2 + Math.floor(random() * 7);
        const start = x;
        for (let letter = 0; letter < letters; letter++) {
          // An n: two stems and an arch; every fourth letter an l, with an ascender.
          const tall = letter % 4 === 3;
          ink(x, baseline - (tall ? xHeight * 1.5 : xHeight), x + stroke, baseline);
          ink(x, baseline - xHeight, x + 10, baseline - xHeight + stroke);
          ink(x + 10 - stroke, baseline - xHeight, x + 10, baseline);
          x += 13;
        }
        words.push([start / width, (x - 3) / width]);
        x += 16;
      }
      lines.push({ v: baseline / height, words });
    }
    pages.push(lines);
    images.push(Buffer.from(pixels));
  }

  const parts = [];
  const offsets = [];
  let length = 0;
  const push = (chunk) => {
    const buffer = typeof chunk === 'string' ? Buffer.from(chunk, 'latin1') : chunk;
    parts.push(buffer);
    length += buffer.length;
  };
  const objects = [];
  const add = (body) => {
    objects.push(body);
    return objects.length;
  };
  const catalog = add(null);
  const pagesNode = add(null);
  const kids = [];
  images.forEach((pixels) => {
    const compressed = deflateSync(pixels);
    const image = add({ head: `<< /Type /XObject /Subtype /Image /Width ${width} /Height ${height} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length ${compressed.length} >>`, stream: compressed });
    const content = 'q 612 0 0 792 0 0 cm /Im0 Do Q';
    const contents = add(`<< /Length ${content.length} >>\nstream\n${content}\nendstream`);
    kids.push(add(`<< /Type /Page /Parent ${pagesNode} 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Im0 ${image} 0 R >> >> /Contents ${contents} 0 R >>`));
  });
  objects[catalog - 1] = `<< /Type /Catalog /Pages ${pagesNode} 0 R >>`;
  objects[pagesNode - 1] = `<< /Type /Pages /Kids [${kids.map((kid) => `${kid} 0 R`).join(' ')}] /Count ${kids.length} >>`;
  push('%PDF-1.7\n%\xe2\xe3\xcf\xd3\n');
  objects.forEach((object, index) => {
    offsets.push(length);
    push(`${index + 1} 0 obj\n`);
    if (typeof object === 'string') {
      push(object);
    } else {
      push(`${object.head}\nstream\n`);
      push(object.stream);
      push('\nendstream');
    }
    push('\nendobj\n');
  });
  const xref = length;
  push(`xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`);
  for (const offset of offsets) {
    push(`${String(offset).padStart(10, '0')} 00000 n \n`);
  }
  push(`trailer\n<< /Size ${objects.length + 1} /Root ${catalog} 0 R >>\nstartxref\n${xref}\n%%EOF\n`);
  return { pdf: Buffer.concat(parts), pages, xHeight: xHeight / height };
}
