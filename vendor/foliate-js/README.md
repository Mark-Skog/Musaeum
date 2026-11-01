# foliate-js

Parte do [foliate-js](https://github.com/johnfactotum/foliate-js), de John Factotum, sob licença
MIT. O código entra no repositório e fica atrás de `apps/desktop/src/reader/foliate-adapter.ts`:
nada mais no projeto importa estes arquivos, então trocar ou reescrever este motor não toca no
resto do app.

| Item | Valor |
|---|---|
| Commit | `78914aef4466eb960965702401634c2cb348e9b1` (2026-05-01) |
| SHA-256 do tarball desse commit | `4878cd5f04073af5046ea543da73a963a7f9e1a4b30ddc63ac86cea5adf02f38` |
| Arquivos | `epub.js`, `epubcfi.js`, `paginator.js`, `vendor/zip.js`, `LICENSE` |
| `vendor/zip.js` | Empacotamento do [zip.js](https://github.com/gildas-lormeau/zip.js), sob BSD-3-Clause; o texto da licença está em `vendor/zip.js.LICENSE` |

Só entrou o necessário para EPUB. Os demais formatos do projeto original ficam de fora: MOBI,
FB2 e similares passam pelo normalizador em Rust previsto na Fase 3, e PDF usa o PDFium.

## Alterações em relação ao original

Uma só, em `paginator.js`, marcada com `MUSAEUM PATCH`:

- O original cria os iframes das seções sempre com `sandbox="allow-same-origin allow-scripts"`,
  por causa do [bug 218086 do WebKit](https://bugs.webkit.org/show_bug.cgi?id=218086). O valor
  agora vem de `sectionFrame.sandbox`, que por padrão não permite scripts. No Chromium isso faz
  o sandbox do iframe valer como barreira independente da CSP.
- Quem rodar sobre WebKit precisa devolver `allow-scripts` e fica só com a CSP.
