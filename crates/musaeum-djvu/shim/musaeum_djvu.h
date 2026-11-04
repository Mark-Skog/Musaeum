/* The narrow C interface between Rust and DjVuLibre.
 *
 * DjVuLibre's own C API, ddjvuapi.h, is built around a message loop, macros and a union of
 * message structures. This file hides all of that behind calls that either finish their work or
 * fail, so the Rust side declares a handful of plain functions and nothing else. */

#ifndef MUSAEUM_DJVU_H
#define MUSAEUM_DJVU_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct mdj_doc mdj_doc;

/* Why mdj_open failed. */
enum {
  MDJ_OPEN_OK = 0,
  MDJ_OPEN_NO_CONTEXT = 1,   /* DjVuLibre could not start */
  MDJ_OPEN_UNREADABLE = 2,   /* not DjVu, or damaged */
  MDJ_OPEN_INDIRECT = 3,     /* its pages are other files, which this process cannot open */
};

/* Opens a document held in memory. DjVuLibre copies the bytes, so they need not outlive the call.
 * Returns NULL and sets *why on failure. */
mdj_doc *mdj_open(const unsigned char *data, size_t len, int *why);

/* Closes the document and everything it keeps. */
void mdj_close(mdj_doc *doc);

/* Number of pages. */
int mdj_page_count(mdj_doc *doc);

/* A page's size in pixels as shown, its resolution, and how many quarter turns counter-clockwise
 * it is shown at. Decodes the page's header only. Returns 0, or -1 if it cannot be read. */
int mdj_page_info(mdj_doc *doc, int page, int *width, int *height, int *dpi, int *rotation);

/* Draws the part (x, y, w, h) of the page, scaled as a whole to page_width × page_height pixels,
 * into RGBA rows of `stride` bytes, top row first. With keep, the decoded page stays for the next
 * call. Returns 0, -1 if the page cannot be decoded, -2 if it cannot be drawn. */
int mdj_render(mdj_doc *doc, int page, int keep, int page_width, int page_height, int x, int y,
               int w, int h, unsigned char *rgba, size_t stride);

/* Called for each word of a text layer, in reading order, with its box in pixels of the page as
 * stored (before rotation), from the bottom-left corner. A NULL text marks the end of a line. */
typedef void (*mdj_word_fn)(void *context, const char *text, int x0, int y0, int x1, int y1);

/* Reads a page's text layer. Returns 0 with no calls if there is none, -1 if it cannot be read. */
int mdj_text(mdj_doc *doc, int page, mdj_word_fn word, void *context);

/* Releases the decoded pages kept by mdj_render. */
void mdj_trim(mdj_doc *doc);

/* One word of a page to encode, in pixels from the page's top-left corner as stored. */
typedef struct {
  int x0, y0, x1, y1;
  const char *text;
  int line_end;
} mdj_word;

/* One page to encode. rgb and mask are optional: width × height pixels, rows from the top; a
 * mask byte is non-zero where the page is black. rotation counts quarter turns counter-clockwise. */
typedef struct {
  int width, height, dpi, rotation;
  const unsigned char *rgb;
  const unsigned char *mask;
  const mdj_word *words;
  int word_count;
} mdj_page;

/* Encodes a bundled document, for test fixtures. On success returns 0 and a buffer to release
 * with mdj_free. Only built with the `encode` feature. */
int mdj_encode(const mdj_page *pages, int count, unsigned char **out, size_t *out_len);
void mdj_free(unsigned char *buffer);

#ifdef __cplusplus
}
#endif

#endif
