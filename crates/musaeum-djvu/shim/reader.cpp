// Reading DjVu documents from memory through DjVuLibre's ddjvuapi, one blocking call at a time.
//
// The decoder runs on threads of its own and reports through messages. Every call here pumps those
// messages until the job it started is over, so nothing is left running between calls. Indirect
// documents, whose pages live in other files, are refused when they open: this process has only
// the bytes the host gave it, and must not go looking for more.

#include <stdlib.h>
#include <string.h>

#include "ddjvuapi.h"
#include "miniexp.h"
#include "musaeum_djvu.h"

namespace {

// Pages kept decoded between calls. Consecutive tiles usually come from the same page, and a facing
// page is often next.
const int CACHED_PAGES = 3;
// The bytes are handed to the decoder in pieces, because its length argument is 32 bits on Windows.
const size_t WRITE_CHUNK = 16u << 20;

struct Cached {
  int pageno;
  ddjvu_page_t *page;
  unsigned long last_used;
};

}  // namespace

struct mdj_doc {
  ddjvu_context_t *context;
  ddjvu_document_t *document;
  ddjvu_format_t *format;
  Cached pages[CACHED_PAGES];
  unsigned long clock;
};

namespace {

// Handles every message waiting, after waiting for one if asked to.
void pump(mdj_doc *doc, bool wait) {
  if (wait)
    ddjvu_message_wait(doc->context);
  const ddjvu_message_t *message;
  while ((message = ddjvu_message_peek(doc->context))) {
    if (message->m_any.tag == DDJVU_NEWSTREAM && message->m_newstream.streamid != 0) {
      // Another file of an indirect document: refused, there is nothing to read it from.
      ddjvu_stream_close(doc->document, message->m_newstream.streamid, 1);
    }
    ddjvu_message_pop(doc->context);
  }
}

void release_page(ddjvu_page_t *page) {
  if (page)
    ddjvu_page_release(page);
}

// A decoded page: from the cache, or decoded now. With keep, a page decoded now goes into the
// cache; otherwise the caller owns it and must release it.
ddjvu_page_t *decoded_page(mdj_doc *doc, int pageno, bool keep, bool *owned) {
  *owned = false;
  doc->clock++;
  for (int i = 0; i < CACHED_PAGES; i++) {
    if (doc->pages[i].page && doc->pages[i].pageno == pageno) {
      doc->pages[i].last_used = doc->clock;
      return doc->pages[i].page;
    }
  }
  ddjvu_page_t *page = ddjvu_page_create_by_pageno(doc->document, pageno);
  if (!page)
    return 0;
  while (!ddjvu_page_decoding_done(page))
    pump(doc, true);
  if (ddjvu_page_decoding_error(page)) {
    ddjvu_page_release(page);
    return 0;
  }
  if (!keep) {
    *owned = true;
    return page;
  }
  int slot = 0;
  for (int i = 0; i < CACHED_PAGES; i++) {
    if (!doc->pages[i].page) {
      slot = i;
      break;
    }
    if (doc->pages[i].last_used < doc->pages[slot].last_used)
      slot = i;
  }
  release_page(doc->pages[slot].page);
  doc->pages[slot].page = page;
  doc->pages[slot].pageno = pageno;
  doc->pages[slot].last_used = doc->clock;
  return page;
}

// Reports every word below `zone`, an expression (type x0 y0 x1 y1 children...), in order.
void walk(miniexp_t zone, mdj_word_fn word, void *context, int depth) {
  // Real text layers nest six levels, from the page to its characters.
  if (!miniexp_consp(zone) || depth > 16)
    return;
  miniexp_t type = miniexp_car(zone);
  miniexp_t rest = miniexp_cdr(zone);
  int box[4];
  for (int i = 0; i < 4; i++) {
    if (!miniexp_consp(rest) || !miniexp_numberp(miniexp_car(rest)))
      return;
    box[i] = miniexp_to_int(miniexp_car(rest));
    rest = miniexp_cdr(rest);
  }
  for (; miniexp_consp(rest); rest = miniexp_cdr(rest)) {
    miniexp_t child = miniexp_car(rest);
    if (miniexp_stringp(child))
      word(context, miniexp_to_str(child), box[0], box[1], box[2], box[3]);
    else
      walk(child, word, context, depth + 1);
  }
  if (type == miniexp_symbol("line"))
    word(context, 0, 0, 0, 0, 0);
}

}  // namespace

extern "C" {

mdj_doc *mdj_open(const unsigned char *data, size_t len, int *why) {
  try {
    mdj_doc *doc = static_cast<mdj_doc *>(calloc(1, sizeof(mdj_doc)));
    if (!doc) {
      *why = MDJ_OPEN_NO_CONTEXT;
      return 0;
    }
    doc->context = ddjvu_context_create("musaeum");
    if (!doc->context) {
      free(doc);
      *why = MDJ_OPEN_NO_CONTEXT;
      return 0;
    }
    // No URL: the document's bytes come through stream 0, and nothing is ever fetched by name.
    doc->document = ddjvu_document_create(doc->context, 0, 0);
    if (!doc->document) {
      mdj_close(doc);
      *why = MDJ_OPEN_NO_CONTEXT;
      return 0;
    }
    for (size_t at = 0; at < len; at += WRITE_CHUNK) {
      size_t piece = len - at < WRITE_CHUNK ? len - at : WRITE_CHUNK;
      ddjvu_stream_write(doc->document, 0, reinterpret_cast<const char *>(data + at),
                         static_cast<unsigned long>(piece));
    }
    ddjvu_stream_close(doc->document, 0, 0);
    while (!ddjvu_document_decoding_done(doc->document))
      pump(doc, true);
    if (ddjvu_document_decoding_error(doc->document)) {
      mdj_close(doc);
      *why = MDJ_OPEN_UNREADABLE;
      return 0;
    }
    ddjvu_document_type_t type = ddjvu_document_get_type(doc->document);
    if (type == DDJVU_DOCTYPE_INDIRECT || type == DDJVU_DOCTYPE_OLD_INDEXED) {
      mdj_close(doc);
      *why = MDJ_OPEN_INDIRECT;
      return 0;
    }
    // RGBA in memory: red in the lowest byte; the fourth mask turns alpha fully on.
    unsigned int masks[4] = {0x000000ffu, 0x0000ff00u, 0x00ff0000u, 0xff000000u};
    doc->format = ddjvu_format_create(DDJVU_FORMAT_RGBMASK32, 4, masks);
    if (!doc->format) {
      mdj_close(doc);
      *why = MDJ_OPEN_NO_CONTEXT;
      return 0;
    }
    ddjvu_format_set_row_order(doc->format, 1);
    ddjvu_format_set_y_direction(doc->format, 1);
    *why = MDJ_OPEN_OK;
    return doc;
  } catch (...) {
    *why = MDJ_OPEN_UNREADABLE;
    return 0;
  }
}

void mdj_close(mdj_doc *doc) {
  if (!doc)
    return;
  try {
    mdj_trim(doc);
    if (doc->format)
      ddjvu_format_release(doc->format);
    if (doc->document)
      ddjvu_document_release(doc->document);
    if (doc->context)
      ddjvu_context_release(doc->context);
  } catch (...) {
  }
  free(doc);
}

int mdj_page_count(mdj_doc *doc) {
  try {
    return ddjvu_document_get_pagenum(doc->document);
  } catch (...) {
    return 0;
  }
}

int mdj_page_info(mdj_doc *doc, int page, int *width, int *height, int *dpi, int *rotation) {
  try {
    ddjvu_pageinfo_t info;
    ddjvu_status_t status;
    while ((status = ddjvu_document_get_pageinfo(doc->document, page, &info)) < DDJVU_JOB_OK)
      pump(doc, true);
    if (status >= DDJVU_JOB_FAILED)
      return -1;
    *width = info.width;
    *height = info.height;
    *dpi = info.dpi;
    *rotation = info.rotation & 3;
    return 0;
  } catch (...) {
    return -1;
  }
}

int mdj_render(mdj_doc *doc, int page, int keep, int page_width, int page_height, int x, int y,
               int w, int h, unsigned char *rgba, size_t stride) {
  try {
    bool owned = false;
    ddjvu_page_t *decoded = decoded_page(doc, page, keep != 0, &owned);
    if (!decoded)
      return -1;
    ddjvu_rect_t whole = {0, 0, static_cast<unsigned int>(page_width),
                          static_cast<unsigned int>(page_height)};
    ddjvu_rect_t part = {x, y, static_cast<unsigned int>(w), static_cast<unsigned int>(h)};
    int drawn = ddjvu_page_render(decoded, DDJVU_RENDER_COLOR, &whole, &part, doc->format,
                                  static_cast<unsigned long>(stride),
                                  reinterpret_cast<char *>(rgba));
    if (owned)
      ddjvu_page_release(decoded);
    if (!drawn) {
      // Nothing to draw, as on a page with no image layers: it is blank paper.
      for (int row = 0; row < h; row++)
        memset(rgba + row * stride, 0xff, static_cast<size_t>(w) * 4);
    }
    return 0;
  } catch (...) {
    return -2;
  }
}

int mdj_text(mdj_doc *doc, int page, mdj_word_fn word, void *context) {
  try {
    miniexp_t text;
    while ((text = ddjvu_document_get_pagetext(doc->document, page, "word")) == miniexp_dummy)
      pump(doc, true);
    if (text == miniexp_nil)
      return 0;
    walk(text, word, context, 0);
    ddjvu_miniexp_release(doc->document, text);
    return 0;
  } catch (...) {
    return -1;
  }
}

void mdj_trim(mdj_doc *doc) {
  for (int i = 0; i < CACHED_PAGES; i++) {
    release_page(doc->pages[i].page);
    doc->pages[i].page = 0;
  }
}

}  // extern "C"
