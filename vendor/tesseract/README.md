# Tesseract 5.5.4, Leptonica 1.87.0 e os modelos `por` e `eng`

O OCR das páginas escaneadas. `crates/musaeum-tesseract` compila as duas bibliotecas com o crate
`cc` e as liga estaticamente ao worker `musaeum-ocr.exe`, que roda isolado (`LockdownParser`, como os
workers de renderização) e é o único programa do Musaeum que contém o Tesseract. Nada aqui foi
modificado.

Esta pasta não vai para o repositório (são cerca de 17 MB de C e C++ e 6 MB de modelos): o
`tools/fetch-ocr.ps1` a baixa e confere cada arquivo pelo SHA-256 registrado no primeiro download. Uma
mudança em qualquer arquivo para o script em vez de trocar o motor em silêncio.

## Origem

| Arquivo | De | SHA-256 |
| --- | --- | --- |
| `tesseract-5.5.4.tar.gz` | <https://github.com/tesseract-ocr/tesseract/archive/refs/tags/5.5.4.tar.gz> | `a8dbf8b8c8ee427ff6ad1d99116502feaf59440af1adb893caf60a6576826cf4` |
| `leptonica-1.87.0.tar.gz` | <https://github.com/DanBloomberg/leptonica/releases/download/1.87.0/leptonica-1.87.0.tar.gz> | `c73363397f96eb1295602bf44d708a994ad42046c791bf03ea0505d829bdb6a7`, igual ao que o GitHub publica |
| `tessdata/por.traineddata` | `tessdata_fast`, etiqueta 4.1.0 | `c4932b937207a9514b7514d518b931a99938c02a28a5a5a553f8599ed58b7deb` |
| `tessdata/eng.traineddata` | `tessdata_fast`, etiqueta 4.1.0 | `7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2` |
| `tessdata/LICENSE` | `tessdata_fast`, etiqueta 4.1.0 | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |

## Licenças

- Tesseract: Apache-2.0 (`tesseract-5.5.4/LICENSE`).
- Leptonica: BSD de duas cláusulas (`leptonica-1.87.0/leptonica-license.txt`).
- Modelos `tessdata_fast`: Apache-2.0 (`tessdata/LICENSE`).

As três estão em `vendor/third-party.json`, que o `tools/check-licenses.mjs` confere; todas são
compatíveis com a GPL-3.0-or-later do Musaeum. Uma distribuição leva os três arquivos de licença.

## Como é compilado

- Leptonica sem nenhuma biblioteca de imagem: o Tesseract só recebe pixels da memória, por isso os
  stubs de leitura de arquivos que o Leptonica tem bastam.
- Tesseract como o seu CMake o faz com `DISABLED_LEGACY_ENGINE`, `GRAPHICS_DISABLED` e `FAST_FLOAT`:
  só o reconhecedor LSTM, sem janela de depuração, redes em precisão simples. A lista de fontes é lida
  do próprio `cmake/SourceLists.cmake`. Os kernels SIMD (SSE4.1, AVX, AVX2, FMA, AVX-512) são
  compilados com os seus conjuntos de instruções e o Tesseract escolhe em tempo de execução.
- `TESSERACT_IMAGEDATA_AS_PIX` mantém as imagens em memória: nada passa por um arquivo temporário.
- Os cabeçalhos que o CMake geraria (`config_auto.h`, `version.h`, `endianness.h`) são escritos pelo
  `build.rs`.
- Os modelos não são abertos pelo worker: o host os lê de `tessdata/` ao lado do executável (o
  `build.rs` do worker os copia para lá) e envia os bytes; o Tesseract os carrega da memória.
