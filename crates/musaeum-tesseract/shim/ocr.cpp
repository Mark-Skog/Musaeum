// The bridge between Rust and Tesseract. Nothing here opens a file: the models come from memory,
// through the reader Tesseract asks for `<datapath>/<language>.traineddata`, and pictures come as
// pixels. Every C++ exception stops here.

#include "musaeum_ocr.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

#include <allheaders.h>
#include <tesseract/baseapi.h>
#include <tesseract/resultiterator.h>

struct MusaeumOcr {
  tesseract::TessBaseAPI api;
};

namespace {

std::mutex models_mutex;
std::map<std::string, std::vector<char>> &models() {
  static std::map<std::string, std::vector<char>> kept;
  return kept;
}

const char kSuffix[] = ".traineddata";

// Tesseract's FileReader: the model whose language the file name ends with.
bool read_model(const char *filename, std::vector<char> *data) {
  if (filename == nullptr || data == nullptr) {
    return false;
  }
  std::string name(filename);
  const size_t slash = name.find_last_of("/\\");
  if (slash != std::string::npos) {
    name = name.substr(slash + 1);
  }
  const size_t suffix = sizeof(kSuffix) - 1;
  if (name.size() <= suffix || name.compare(name.size() - suffix, suffix, kSuffix) != 0) {
    return false;
  }
  name.resize(name.size() - suffix);
  std::lock_guard<std::mutex> lock(models_mutex);
  const auto found = models().find(name);
  if (found == models().end()) {
    return false;
  }
  *data = found->second;
  return true;
}

void set_error(char *error, size_t size, const char *message) {
  if (error == nullptr || size == 0) {
    return;
  }
  std::snprintf(error, size, "%s", message);
}

void put_u32(std::string &out, uint32_t value) {
  out.append(reinterpret_cast<const char *>(&value), sizeof value);
}

void put_i32(std::string &out, int32_t value) {
  out.append(reinterpret_cast<const char *>(&value), sizeof value);
}

void put_f32(std::string &out, float value) {
  out.append(reinterpret_cast<const char *>(&value), sizeof value);
}

}  // namespace

extern "C" int musaeum_ocr_add_model(const char *language, const uint8_t *data, size_t size) {
  try {
    if (language == nullptr || data == nullptr || size == 0) {
      return 1;
    }
    std::lock_guard<std::mutex> lock(models_mutex);
    models()[language] = std::vector<char>(reinterpret_cast<const char *>(data),
                                           reinterpret_cast<const char *>(data) + size);
    return 0;
  } catch (...) {
    return 2;
  }
}

extern "C" MusaeumOcr *musaeum_ocr_create(const char *languages, char *error, size_t error_size) {
  try {
    // Leptonica reports what it lacks without image libraries (a bitmap font it never needs here)
    // on every start; Tesseract's own messages still go to stderr.
    setMsgSeverity(L_SEVERITY_NONE);
    auto engine = std::make_unique<MusaeumOcr>();
    // A data path that is never opened: the reader serves every file under it.
    if (engine->api.Init("mem", 0, languages, tesseract::OEM_LSTM_ONLY, nullptr, 0, nullptr,
                         nullptr, false, read_model) != 0) {
      set_error(error, error_size, "Tesseract could not load the models for these languages");
      return nullptr;
    }
    // Pictures come black on light; trying every word inverted too only costs time.
    engine->api.SetVariable("tessedit_do_invert", "0");
    return engine.release();
  } catch (const std::exception &caught) {
    set_error(error, error_size, caught.what());
  } catch (...) {
    set_error(error, error_size, "Tesseract failed while loading");
  }
  return nullptr;
}

extern "C" void musaeum_ocr_destroy(MusaeumOcr *engine) {
  try {
    delete engine;
  } catch (...) {
  }
}

extern "C" int musaeum_ocr_recognize(MusaeumOcr *engine, const uint8_t *gray, int width, int height,
                                     int stride, int psm, int ppi, uint8_t **out, size_t *out_size,
                                     char *error, size_t error_size) {
  if (engine == nullptr || gray == nullptr || out == nullptr || out_size == nullptr || width <= 0 ||
      height <= 0 || stride < width) {
    set_error(error, error_size, "nothing to recognize");
    return 1;
  }
  *out = nullptr;
  *out_size = 0;
  try {
    tesseract::TessBaseAPI &api = engine->api;
    api.SetPageSegMode(static_cast<tesseract::PageSegMode>(psm));
    api.SetImage(gray, width, height, 1, stride);
    api.SetSourceResolution(ppi > 0 ? ppi : 300);
    if (api.Recognize(nullptr) != 0) {
      api.Clear();
      set_error(error, error_size, "Tesseract could not recognize the picture");
      return 2;
    }
    std::string buffer;
    put_u32(buffer, 0);
    uint32_t words = 0;
    std::unique_ptr<tesseract::ResultIterator> it(api.GetIterator());
    if (it != nullptr) {
      int block = -1;
      int paragraph = -1;
      int line = -1;
      const tesseract::PageIteratorLevel level = tesseract::RIL_WORD;
      do {
        if (it->Empty(level)) {
          continue;
        }
        if (it->IsAtBeginningOf(tesseract::RIL_BLOCK)) {
          block++;
        }
        if (it->IsAtBeginningOf(tesseract::RIL_PARA)) {
          paragraph++;
        }
        if (it->IsAtBeginningOf(tesseract::RIL_TEXTLINE)) {
          line++;
        }
        std::unique_ptr<char[]> text(it->GetUTF8Text(level));
        if (text == nullptr) {
          continue;
        }
        int left = 0, top = 0, right = 0, bottom = 0;
        it->BoundingBox(level, &left, &top, &right, &bottom);
        int x1 = left, y1 = bottom, x2 = right, y2 = bottom;
        it->Baseline(level, &x1, &y1, &x2, &y2);
        put_i32(buffer, left);
        put_i32(buffer, top);
        put_i32(buffer, right);
        put_i32(buffer, bottom);
        put_f32(buffer, it->Confidence(level));
        put_u32(buffer, static_cast<uint32_t>(block < 0 ? 0 : block));
        put_u32(buffer, static_cast<uint32_t>(paragraph < 0 ? 0 : paragraph));
        put_u32(buffer, static_cast<uint32_t>(line < 0 ? 0 : line));
        put_i32(buffer, x1);
        put_i32(buffer, y1);
        put_i32(buffer, x2);
        put_i32(buffer, y2);
        const size_t length = std::strlen(text.get());
        put_u32(buffer, static_cast<uint32_t>(length));
        buffer.append(text.get(), length);
        words++;
      } while (it->Next(level));
    }
    std::memcpy(&buffer[0], &words, sizeof words);
    api.Clear();
    auto *copy = static_cast<uint8_t *>(std::malloc(buffer.size()));
    if (copy == nullptr) {
      set_error(error, error_size, "out of memory");
      return 3;
    }
    std::memcpy(copy, buffer.data(), buffer.size());
    *out = copy;
    *out_size = buffer.size();
    return 0;
  } catch (const std::exception &caught) {
    set_error(error, error_size, caught.what());
  } catch (...) {
    set_error(error, error_size, "Tesseract failed while recognizing");
  }
  try {
    engine->api.Clear();
  } catch (...) {
  }
  return 4;
}

extern "C" void musaeum_ocr_free(uint8_t *buffer) { std::free(buffer); }
