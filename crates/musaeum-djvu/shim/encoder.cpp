// Writes bundled DjVu documents for test fixtures, the way DjVuLibre's own tools do: c44 for the
// color layer, cjb2 for the black one, djvused for the text layer and djvm to bundle the pages.
// Only test code calls this; the worker is built without it.

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "BSByteStream.h"
#include "ByteStream.h"
#include "DjVmDoc.h"
#include "DjVuInfo.h"
#include "DjVuPalette.h"
#include "DjVuText.h"
#include "GBitmap.h"
#include "GPixmap.h"
#include "GRect.h"
#include "IFFByteStream.h"
#include "IW44Image.h"
#include "JB2Image.h"
#include "musaeum_djvu.h"

#ifdef HAVE_NAMESPACES
using namespace DJVU;
#endif

namespace {

// DjVu counts rows from the bottom; these pages count them from the top.
void write_mask(IFFByteStream &iff, const mdj_page &page) {
  GP<GBitmap> bits = GBitmap::create(page.height, page.width);
  for (int y = 0; y < page.height; y++) {
    unsigned char *row = (*bits)[page.height - 1 - y];
    for (int x = 0; x < page.width; x++)
      row[x] = page.mask[y * page.width + x] ? 1 : 0;
  }
  GP<JB2Image> jb2 = JB2Image::create();
  jb2->set_dimension(page.width, page.height);
  JB2Shape shape;
  shape.parent = -1;
  shape.bits = bits;
  JB2Blit blit;
  blit.left = 0;
  blit.bottom = 0;
  blit.shapeno = jb2->add_shape(shape);
  jb2->add_blit(blit);
  iff.put_chunk("Sjbz");
  jb2->encode(iff.get_bytestream());
  iff.close_chunk();
  if (!page.rgb)
    return;
  // Over a color layer, DjVuLibre only composes the page if the black layer has a color too, as
  // cpaldjvu writes it: a palette of one, black, for the one blit.
  GP<DjVuPalette> palette = DjVuPalette::create();
  palette->histogram_clear();
  GPixel black;
  black.b = black.g = black.r = 0;
  palette->histogram_add(black, 1);
  palette->compute_palette(1);
  palette->colordata.touch(0);
  palette->colordata[0] = 0;
  iff.put_chunk("FGbz");
  palette->encode(iff.get_bytestream());
  iff.close_chunk();
}

void write_background(IFFByteStream &iff, const mdj_page &page) {
  GP<GPixmap> pixels = GPixmap::create(page.height, page.width);
  for (int y = 0; y < page.height; y++) {
    GPixel *row = (*pixels)[page.height - 1 - y];
    const unsigned char *source = page.rgb + static_cast<size_t>(y) * page.width * 3;
    for (int x = 0; x < page.width; x++) {
      row[x].r = source[x * 3];
      row[x].g = source[x * 3 + 1];
      row[x].b = source[x * 3 + 2];
    }
  }
  GP<IW44Image> wavelets = IW44Image::create_encode(*pixels, 0, IW44Image::CRCBnormal);
  // c44's default: three chunks, refining the image as more of the file arrives.
  const int slices[3] = {74, 89, 99};
  for (int i = 0; i < 3; i++) {
    IWEncoderParms parms;
    parms.slices = slices[i];
    iff.put_chunk("BG44");
    int more = wavelets->encode_chunk(iff.get_bytestream(), parms);
    iff.close_chunk();
    if (!more)
      break;
  }
  wavelets->close_codec();
}

void write_text(IFFByteStream &iff, const mdj_page &page) {
  GP<DjVuTXT> text = DjVuTXT::create();
  text->page_zone.ztype = DjVuTXT::PAGE;
  text->page_zone.rect = GRect(0, 0, page.width, page.height);
  DjVuTXT::Zone *line = 0;
  for (int i = 0; i < page.word_count; i++) {
    const mdj_word &source = page.words[i];
    GRect box;
    box.xmin = source.x0;
    box.xmax = source.x1;
    box.ymin = page.height - source.y1;
    box.ymax = page.height - source.y0;
    if (!line) {
      line = text->page_zone.append_child();
      line->ztype = DjVuTXT::LINE;
      line->rect = box;
    } else {
      line->rect.recthull(GRect(line->rect), box);
    }
    DjVuTXT::Zone *word = line->append_child();
    word->ztype = DjVuTXT::WORD;
    word->rect = box;
    word->text_start = text->textUTF8.length();
    word->text_length = static_cast<int>(strlen(source.text));
    text->textUTF8 += source.text;
    if (source.line_end)
      line = 0;
  }
  // Puts the separators between words and lines into the text, as djvused does.
  text->normalize_text();
  iff.put_chunk("TXTz");
  {
    GP<ByteStream> compressed = BSByteStream::create(iff.get_bytestream(), 1000);
    text->encode(compressed);
  }
  iff.close_chunk();
}

GP<ByteStream> encode_page(const mdj_page &page) {
  GP<ByteStream> bytes = ByteStream::create();
  GP<IFFByteStream> giff = IFFByteStream::create(bytes);
  IFFByteStream &iff = *giff;
  iff.put_chunk("FORM:DJVU", 1);
  GP<DjVuInfo> info = DjVuInfo::create();
  info->width = page.width;
  info->height = page.height;
  info->dpi = page.dpi;
  info->gamma = 2.2;
  info->orientation = page.rotation & 3;
  iff.put_chunk("INFO");
  info->encode(*iff.get_bytestream());
  iff.close_chunk();
  if (page.mask)
    write_mask(iff, page);
  if (page.rgb)
    write_background(iff, page);
  if (page.word_count > 0)
    write_text(iff, page);
  iff.close_chunk();
  bytes->seek(0);
  return bytes;
}

}  // namespace

extern "C" {

int mdj_encode(const mdj_page *pages, int count, unsigned char **out, size_t *out_len) {
  try {
    GP<DjVmDoc> document = DjVmDoc::create();
    for (int i = 0; i < count; i++) {
      GP<ByteStream> page = encode_page(pages[i]);
      char name[32];
      snprintf(name, sizeof(name), "p%04d.djvu", i + 1);
      document->insert_file(*page, DjVmDir::File::PAGE, name, name);
    }
    GP<ByteStream> bundle = ByteStream::create();
    document->write(bundle);
    size_t len = static_cast<size_t>(bundle->size());
    unsigned char *buffer = static_cast<unsigned char *>(malloc(len));
    if (!buffer)
      return -1;
    bundle->seek(0);
    bundle->readall(buffer, len);
    *out = buffer;
    *out_len = len;
    return 0;
  } catch (...) {
    return -1;
  }
}

void mdj_free(unsigned char *buffer) {
  free(buffer);
}

}  // extern "C"
