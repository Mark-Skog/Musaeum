# Musaeum

Leitor e gerenciador de livros digitais, livre e de código aberto (GPL-3.0-or-later).

O projeto está na **Fase 0 do Plano Diretor**: validar a stack com medições antes de
construir o produto. O que existe hoje é o pipeline de layout fixo, do PDF até a tela, e o
harness que o mede. Ainda não há biblioteca, EPUB ou interface de produto.

Por enquanto, o código só compila e roda no **Windows**.

## Estrutura

```text
crates/
  musaeum-shm/         Protocolo de memória compartilhada: slab de pixels, anel de entrega,
                       mensagens de controle e aritmética de tiles
  musaeum-core/        Agendador (máquina de estados pura) e supervisor dos workers,
                       sem dependência de Tauri
  musaeum-render-pdf/  Processo isolado que rasteriza tiles com PDFium
apps/desktop/
  src-tauri/           Ponte Tauri: protocolo tile://, buffers do WebView2, harness
  src/                 Thread principal e compositor WebGL2 em Web Worker
tools/
  fetch-pdfium.ps1     Baixa a build fixada do PDFium e confere o SHA-256
  gen-fixtures/        Gera os PDFs de teste
  pool-bench/          Mede o pool de render sem WebView
  run-bench.mjs        Roda a matriz dos Experimentos A e B
  test-cascade.mjs     Testa a escolha automática do caminho de entrega no app real
```

## Como rodar

Pré-requisitos: Rust (a versão é fixada em `rust-toolchain.toml`), Visual Studio Build Tools
com C++, Node.js e o runtime do WebView2, que já vem no Windows 11.

```powershell
powershell -ExecutionPolicy Bypass -File tools\fetch-pdfium.ps1

cd apps\desktop
npm install
npm run build:app
cd ..\..

.\target\release\gen-fixtures.exe --out fixtures
```

Abrir um PDF de forma interativa (a roda do mouse faz a rolagem):

```powershell
.\target\release\musaeum-desktop.exe --pdf fixtures\scan-2000.pdf
```

Rodar a matriz de medição. Ela abre janelas em primeiro plano, uma por vez, e escreve os
relatórios em `bench-results/`:

```powershell
node tools\run-bench.mjs
node tools\run-bench.mjs --scenario quick --uncapped --stress 0
```

## Os caminhos de entrega de pixels

Existem três formas de levar um tile finalizado até o compositor. O resto do pipeline é
idêntico, então o caminho de entrega é a única variável do Experimento B.

| Caminho | Como o tile chega ao Worker | Depende da thread principal? |
|---|---|---|
| A | `fetch` no protocolo `tile://` | Não |
| B | Thread principal copia do SharedBuffer para um anel de SharedArrayBuffer | Sim |
| C | O Worker recebe os SharedBuffers por transferência e lê no próprio lugar | Não |

O Caminho C depende de um comportamento que o WebView2 não documenta: a transferência do
`ArrayBuffer` para um Worker manter a memória viva. Por isso, a cada início, o app testa o
runtime em buffers pequenos (`src/probe.ts`) e escolhe o melhor caminho que funciona
(`src/delivery-select.ts`):

1. **C**, se a memória transferida continuar viva nos dois sentidos.
2. **B**, se o teste falhar e a página tiver isolamento de origem.
3. **A**, sem SharedBuffer do WebView2 ou como último recurso.

Um caminho que falhe ao abrir a sessão também passa a vez ao seguinte. Um documento corrompido
não troca de caminho, porque falharia em todos.

`--delivery a|b|c` força um caminho, para medi-lo, e falha em vez de recair em outro. O padrão
é `--delivery auto`.

## Testes

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd apps\desktop; npm test; npx tsc --noEmit
```

Os testes de integração em `crates/musaeum-render-pdf/tests/` usam processos reais, memória
compartilhada real e o PDFium real, então exigem o `fetch-pdfium.ps1` antes.

A escolha do caminho é testada de ponta a ponta no app compilado. As falhas são simuladas com
`--delivery-fault`, e cada caso exige que os tiles cheguem à tela pelo caminho esperado:

```powershell
node tools\test-cascade.mjs
```
