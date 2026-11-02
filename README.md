# Musaeum

Leitor e gerenciador de livros digitais, livre e de código aberto (GPL-3.0-or-later).

O projeto está na **Fase 1 do Plano Diretor**: o núcleo sem interface. O que existe hoje são os dois
pipelines de leitura da Fase 0, o de layout fixo (PDF) e o fluido (EPUB), e a biblioteca: SQLite,
identidade de arquivos, indexador, diário de operações em disco e uma CLI. Ainda não há interface de
produto.

As decisões e os números de cada fase estão em
[`docs/adr/0001-fase-0.md`](docs/adr/0001-fase-0.md) e
[`docs/adr/0002-fase-1.md`](docs/adr/0002-fase-1.md).

Por enquanto, o código só compila e roda no **Windows**.

## Estrutura

```text
crates/
  musaeum-shm/         Protocolo de memória compartilhada: slab de pixels, anel de entrega,
                       mensagens de controle e aritmética de tiles
  musaeum-core/        Agendador (máquina de estados pura) e supervisor dos workers,
                       sem dependência de Tauri
  musaeum-sandbox/     Token restrito, mitigações, job object e estação de janelas isolada
  musaeum-render-pdf/  Processo isolado que rasteriza tiles com PDFium
  musaeum-fs/          File IDs de 128 bits, listagem sem abrir arquivos, leitura que recusa
                       placeholders de nuvem, watcher que avisa quando perde eventos
  musaeum-ingest/      Processo isolado que lê identificadores e metadados dos livros
  musaeum-library/     SQLite, reconhecimento de arquivos, indexador, diário de operações
apps/cli/              O binário `musaeum`: a biblioteca pela linha de comando
apps/desktop/
  src-tauri/           Ponte Tauri: protocolos tile:// e book://, buffers do WebView2, harness
  src/                 Thread principal e compositor WebGL2 em Web Worker
  src/reader/          Leitor fluido, numa origem própria, com o foliate-js atrás de um adaptador
vendor/
  foliate-js/          Parte do foliate-js, fixada em um commit, com uma alteração documentada
  third-party.json     Licenças do que entra no app sem passar por um gerenciador de pacotes
tools/
  fetch-pdfium.ps1     Baixa a build fixada do PDFium e confere o SHA-256
  gen-fixtures/        Gera os PDFs e os EPUBs de teste, incluindo um EPUB hostil
  pool-bench/          Mede o pool de render sem WebView
  library-bench/       Mede a biblioteca com 100 mil arquivos reais, tecla por tecla
  run-bench.mjs        Roda a matriz dos Experimentos A e B
  run-epub-bench.mjs   Mede o tempo de relayout do leitor fluido
  measure-memory.mjs   Mede a RAM do app inteiro
  test-cascade.mjs     Testa a escolha automática do caminho de entrega no app real
  test-epub-security.mjs  Abre o EPUB hostil no app real, com cada barreira isolada
  check-licenses.mjs   Confere as licenças de todas as dependências
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

Abrir um documento de forma interativa. No PDF, a roda do mouse faz a rolagem. No EPUB, as
setas passam as páginas:

```powershell
.\target\release\musaeum-desktop.exe --pdf fixtures\scan-2000.pdf
.\target\release\musaeum-desktop.exe --epub fixtures\large.epub
```

Rodar as medições. Elas abrem janelas em primeiro plano, uma por vez, e escrevem os relatórios
em `bench-results/`. Rode com a máquina livre: outro programa usando a GPU muda os resultados.

```powershell
node tools\run-bench.mjs
node tools\run-bench.mjs --scenario quick --uncapped --stress 0
node tools\run-epub-bench.mjs
node tools\measure-memory.mjs
```

## A biblioteca pela linha de comando

```powershell
cargo build --release -p musaeum-cli -p musaeum-library

.\target\release\musaeum.exe root add D:\Livros
.\target\release\musaeum.exe reconcile
.\target\release\musaeum.exe status
.\target\release\musaeum.exe search dostoievski
.\target\release\musaeum.exe search "dom casmurro" --keystrokes
.\target\release\musaeum.exe inspect D:\Livros\um-livro.pdf
.\target\release\musaeum.exe revisions D:\Livros\um-livro.pdf
.\target\release\musaeum.exe suggestions
.\target\release\musaeum.exe move D:\Livros\a.pdf D:\Livros\b.pdf
.\target\release\musaeum.exe undo
.\target\release\musaeum.exe watch
```

`musaeum --help` lista tudo, e `--json` antes do comando dá a saída para programas. Os dados ficam em
`%LOCALAPPDATA%\Musaeum`, ou onde `--library DIR` disser. O que só consulta funciona enquanto
`musaeum watch` estiver rodando.

Três regras valem para tudo isso:

1. **O disco é a verdade.** Eventos do sistema só dizem onde olhar primeiro, e um evento perdido custa
   atraso, não dados. A mesma reconciliação cobre a primeira varredura, o tempo real e o que mudou com
   o programa fechado.
2. **Nada é apagado.** Arquivo que some fica ausente, com tudo que está ligado a ele. Triggers no banco
   recusam apagar uma revisão confirmada ou uma anotação, e o disco só muda por ação explícita, com
   diário e desfazer.
3. **Parser só em sandbox.** Ler `/ID[0]` ou `dc:identifier` é interpretar bytes hostis, então quem faz
   isso é `musaeum-ingest`, sem rede e sem acesso a arquivos por caminho, recebendo cada livro como um
   handle. O que ele responde é tratado como vindo de um atacante.

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

## O sandbox do renderizador

O PDF é entrada hostil, então o worker é tratado como se já estivesse comprometido. Ele roda
com um token restrito sem nenhum grupo utilizável, em nível de integridade baixo, com mitigações
de processo, em um job object próprio e em uma estação de janelas separada. O worker carrega o
PDFium, entra em lockdown e só depois lê qualquer mensagem do host. Daí em diante, só tem o que
o host duplicou para ele: um handle somente leitura do documento e a memória compartilhada.

`tests/sandbox.rs` prova isso tentando: `sandbox-probe` se comporta como um worker comprometido,
e cada tentativa negada dentro do sandbox precisa dar certo fora dele.

`--no-sandbox` existe só para medir o custo do sandbox e exige `--bench`.

## O isolamento do leitor fluido

Um livro é entrada hostil também, e o foliate-js não oferece proteção própria contra isso:

1. **CSP com nonce:** só os scripts do leitor têm o nonce. Um `blob:` herda a política da página
   que o criou, então um cabeçalho cobre todas as seções do livro.
2. **Sandbox dos iframes das seções, sem `allow-scripts`:** barreira independente da CSP. Só vale
   no Chromium, porque um bug do WebKit obriga o foliate-js a manter `allow-scripts` lá.
3. **Origem própria por livro:** `http://book.<aleatório>.localhost`. O Tauri trata como local a
   origem `<protocolo>.localhost` de um protocolo registrado, e um script ali conseguiria chamar
   os comandos do app. Com o sufixo aleatório, a origem conta como remota e a ACL recusa a chamada.

`tools/test-epub-security.mjs` testa cada barreira sozinha e, como controle, sem nenhuma.

## Testes

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd apps\desktop; npm test; npx tsc --noEmit; cd ..\..
node --test tools\lib\spdx.test.mjs
node tools\check-licenses.mjs
```

Os testes de integração em `crates/musaeum-render-pdf/tests/` usam processos reais, memória
compartilhada real e o PDFium real, então exigem o `fetch-pdfium.ps1` antes.

A bateria da biblioteca em `crates/musaeum-library/tests/` trabalha com arquivos reais, File IDs reais,
o worker real no sandbox real e eventos reais do sistema. Alguns testes tocam em coisas do sistema, e
sempre desfazem o que fizeram: `cloud.rs` registra uma sync root de verdade com a Cloud Filter API,
`real_volume.rs` monta uma imagem de disco sem letra de unidade, e `journal.rs` usa a Lixeira de
verdade.

O teste de carga gera 100 mil arquivos reais em `%TEMP%` (cerca de 900 MiB, removidos ao final) e escreve
em `bench-results/`:

```powershell
.\target\release\library-bench.exe
```

Dois testes rodam só no app compilado e abrem janelas. A escolha do caminho simula falhas com
`--delivery-fault`, e cada caso exige que os tiles cheguem à tela pelo caminho esperado. O teste
de segurança abre um EPUB hostil e observa de fora o que escapa, com um servidor canário:

```powershell
node tools\test-cascade.mjs
node tools\test-epub-security.mjs
```

## Licenças

O projeto é GPL-3.0-or-later, e `deny.toml` lista as licenças aceitas nas dependências. Qualquer
outra licença reprova a verificação, então uma licença nova exige uma decisão. Em cada clone,
ative o hook que roda a verificação quando uma dependência muda:

```powershell
git config core.hooksPath .githooks
```

A verificação precisa do `cargo-deny` (`cargo install cargo-deny`).
