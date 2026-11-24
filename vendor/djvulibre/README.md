# DjVuLibre 3.5.30, a biblioteca `libdjvu`

Usada por `crates/musaeum-djvu`, que a compila com o crate `cc` junto com uma ponte em C++ e a liga
estaticamente ao worker `musaeum-render-djvu`. Nada aqui foi modificado.

## Origem

- Pacote: `djvulibre-3.5.30.tar.gz`, de
  <https://sourceforge.net/projects/djvu/files/DjVuLibre/3.5.30/> (publicado em 05/05/2026).
- SHA-1 `a07f431c04c33f3685547bbf04e6fdb67c279456` e MD5 `18bd2ed16e66c9238fd84044d11324a6`, iguais
  aos que o SourceForge publica para o arquivo. SHA-256:
  `ee5e457d4cfebe566f94b99e5e3d3cc7f5c79ddb741c2ac2ba2e456f00329644`.
- O projeto não publica assinatura do pacote.

## Licença

GPL-2.0-or-later (`COPYING` e `COPYRIGHT`). O "or later" permite usá-la sob a GPL-3.0, a licença do
Musaeum. O Plano Diretor aprovou o DjVuLibre num processo isolado; `tools/check-licenses.mjs` aceita
essa licença só para o que é vendorizado.

## O que está aqui

- `libdjvu/`: todos os `.cpp` e `.h` da biblioteca, como no pacote. O `build.rs` usa as definições do
  projeto do Visual Studio que vem com o DjVuLibre (`win32/djvulibre/libdjvulibre`), sem as de
  exportação de DLL e sem `NEED_JPEG_DECODER`: páginas com camadas em JPEG (`BGjp`, `FGjp`), raras, não
  são desenhadas.
- `COPYING`, `COPYRIGHT`: a licença e os avisos de copyright.

## Como a biblioteca é usada

- Só documentos embalados (bundled) abrem. Um documento indireto guarda as páginas em outros arquivos,
  e o worker não tem como, nem permissão para, abri-los; a ponte recusa esses documentos ao abrir.
- O documento chega à biblioteca pela API de stream, a partir dos bytes que o host compartilhou. Ela
  nunca recebe um caminho ou URL.
- Para achar seus catálogos de mensagens, o DjVuLibre lê o registro do Windows; dentro do sandbox
  não encontra nada, e as mensagens ficam em inglês.
- Desde a 1.0, o runtime C++ da Microsoft vai dentro do worker (`/MT`, por `.cargo/config.toml`): ele
  não importa mais o `MSVCP140.dll` nem o `VCRUNTIME140_1.dll`, e roda num Windows sem o Visual C++
  Redistributable (ADR 0015).

## Atualizar

Baixar a nova versão, conferir os hashes, substituir `libdjvu/` e rodar
`cargo test -p musaeum-djvu -p musaeum-render-djvu`.
