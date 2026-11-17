// The C side of musaeum-tesseract: a Tesseract engine that reads its models from memory and
// recognizes gray pictures, with every C++ exception caught before it reaches Rust.

#ifndef MUSAEUM_OCR_H
#define MUSAEUM_OCR_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct MusaeumOcr MusaeumOcr;

// Keeps a copy of a model, `<language>.traineddata`, for engines made after. 0 on success.
int musaeum_ocr_add_model(const char *language, const uint8_t *data, size_t size);

// An engine for `languages` ("por", "por+eng"), whose models were added. NULL on failure, with why
// in `error` (always terminated).
MusaeumOcr *musaeum_ocr_create(const char *languages, char *error, size_t error_size);

void musaeum_ocr_destroy(MusaeumOcr *engine);

// Recognizes `gray` (one byte a pixel, 0 black, rows `stride` bytes apart) in page segmentation
// mode `psm` at `ppi` pixels per inch. On success returns 0 and sets `*out` to a buffer of
// `*out_size` bytes that musaeum_ocr_free releases:
//
//   u32 word count, then for every word in reading order:
//   i32 left, top, right, bottom (pixels, right and bottom exclusive)
//   f32 confidence, 0 to 100
//   u32 block, paragraph, line (counting from 0 across the picture)
//   i32 baseline x1, y1, x2, y2 of the word
//   u32 text length, then the text in UTF-8
int musaeum_ocr_recognize(MusaeumOcr *engine, const uint8_t *gray, int width, int height, int stride,
                          int psm, int ppi, uint8_t **out, size_t *out_size, char *error,
                          size_t error_size);

void musaeum_ocr_free(uint8_t *buffer);

#ifdef __cplusplus
}
#endif

#endif
