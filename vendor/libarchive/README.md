# libarchive 3.8.11, só a leitura de ZIP, RAR e RAR5

Usado por `crates/musaeum-archive`, que o compila com o crate `cc` e o liga estaticamente ao worker
`musaeum-render-raster`. Nada aqui foi modificado.

## Origem

- Pacote: `libarchive-3.8.11.tar.xz`, de
  <https://github.com/libarchive/libarchive/releases/tag/v3.8.11> (publicado em 30/10/2026).
- SHA-256: `a3dc0872b1b39da34dc18ef1b944f0f11ffd37425d70ae04be679279f1cf43d2`, igual ao que o GitHub
  publica para o arquivo.
- Assinatura PGP (`.asc`) válida, da chave `659C 84C0 E23E A1FA 97E0 B58C C040 B508 D63D 2B36`, de
  Martin Matuska, que publica as versões do libarchive. A chave veio de keys.openpgp.org.

## O que está aqui

- `COPYING`: a licença. Os arquivos usados são BSD-2-Clause; os de BLAKE2 (`archive_blake2*`) podem
  ser usados como CC0-1.0 ou Apache-2.0.
- `libarchive/`: todos os cabeçalhos e só os `.c` da leitura de ZIP (o leitor que usa o diretório
  central), RAR e RAR5, mais o núcleo que eles chamam. A lista exata está em
  `crates/musaeum-archive/build.rs`. Ficaram de fora a escrita de arquivos, a extração para disco, os
  outros formatos e os filtros que executam programas externos (`archive_read_support_filter_program.c`,
  `filter_fork_windows.c`): um recurso que não foi compilado não pode ser explorado.
- `config/windows/config.h`: gerado pelo CMake 3.31 do Visual Studio Build Tools 2022 (MSVC 14.44)
  com o gerador "Visual Studio 17 2022", x64, e estas opções:

  ```text
  -DENABLE_MBEDTLS=OFF -DENABLE_NETTLE=OFF -DENABLE_OPENSSL=OFF -DENABLE_LIBB2=OFF -DENABLE_LZ4=OFF
  -DENABLE_LZO=OFF -DENABLE_LZMA=OFF -DENABLE_ZSTD=OFF -DENABLE_ZLIB=OFF -DENABLE_BZip2=OFF
  -DENABLE_LIBXML2=OFF -DENABLE_EXPAT=OFF -DENABLE_WIN32_XMLLITE=OFF -DENABLE_PCREPOSIX=OFF
  -DENABLE_PCRE2POSIX=OFF -DENABLE_LIBGCC=OFF -DENABLE_CNG=OFF -DENABLE_TAR=OFF -DENABLE_CPIO=OFF
  -DENABLE_CAT=OFF -DENABLE_UNZIP=OFF -DENABLE_XATTR=OFF -DENABLE_ACL=OFF -DENABLE_ICONV=OFF
  -DENABLE_TEST=OFF -DENABLE_INSTALL=OFF -DPOSIX_REGEX_LIB=NONE
  ```

  O zlib vem do crate `libz-sys`, compilado do fonte; por isso o CMake rodou sem ele, e o
  `build.rs` define as duas macros que ele teria escrito, `HAVE_ZLIB_H` e `HAVE_LIBZ`.

## Limitações conhecidas

- Entradas de ZIP comprimidas com bzip2, LZMA, XZ ou zstd não abrem: as bibliotecas desses métodos não
  foram incluídas. Deflate, PPMd e entradas sem compressão abrem. Quadrinhos usam deflate ou nenhuma
  compressão.
- ZIP e RAR cifrados não abrem: o leitor nunca recebe senha.
- Num RAR sólido, ler uma página exige descomprimir as anteriores.

## Atualizar

Baixar a nova versão, conferir hash e assinatura, copiar os mesmos arquivos de `libarchive/`, gerar o
`config.h` com as mesmas opções e rodar `cargo test -p musaeum-archive -p musaeum-render-raster`.
