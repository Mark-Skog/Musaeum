# Musaeum

Leitor e gerenciador de livros digitais, livre e de código aberto (GPL-3.0-or-later).

O projeto está na **Fase 3 do Plano Diretor**, o layout fluido. O que existe hoje: o normalizador dos
livros fluidos (EPUB 2 e 3, MOBI, AZW3, FB2, TXT e Markdown), que roda num processo isolado e entrega cada
livro como um pacote EPUB 3 inerte, com seções de no máximo 200 KiB, e o motor de posição (EPUB CFI e
Locator); o leitor de layout fixo da Fase 2 para PDF, quadrinhos (CBZ e CBR) e DjVu, com tiles na escala
exata da tela, páginas duplas, modo paginado e contínuo, a posição guardada na biblioteca, a classificação
de cada página e miniaturas em disco; o leitor fluido (EPUB) da Fase 0; e a biblioteca da Fase 1, com
SQLite, identidade de arquivos, indexador, diário de operações em disco e uma CLI. Ainda não há interface
de produto além da barra de leitura.

As decisões e os números de cada fase estão em
[`docs/adr/0001-fase-0.md`](docs/adr/0001-fase-0.md),
[`docs/adr/0002-fase-1.md`](docs/adr/0002-fase-1.md),
[`docs/adr/0003-fase-2-parte-1.md`](docs/adr/0003-fase-2-parte-1.md),
[`docs/adr/0004-fase-2-parte-2.md`](docs/adr/0004-fase-2-parte-2.md) e
[`docs/adr/0005-fase-3-parte-1.md`](docs/adr/0005-fase-3-parte-1.md).

Por enquanto, o código só compila e roda no **Windows**.

## Estrutura

```text
crates/
  musaeum-shm/         Protocolo de memória compartilhada: slab de pixels, anel de entrega,
                       mensagens de controle e aritmética de tiles
  musaeum-core/        Agendador (máquina de estados pura) e supervisor dos workers,
                       sem dependência de Tauri
  musaeum-sandbox/     Token restrito, mitigações, job object e estação de janelas isolada
  musaeum-worker/      O que todo worker de render compartilha: handshake, handles, laço de
                       controle, cancelamento, e o arnês que os testes usam
  musaeum-render-pdf/  Processo isolado que rasteriza tiles com PDFium
  musaeum-archive/     libarchive vendorizada, só a leitura de ZIP, RAR e RAR5
  musaeum-render-raster/  Processo isolado que desenha quadrinhos (CBZ, CBR)
  musaeum-djvu/        DjVuLibre vendorizada, atrás de uma ponte em C++
  musaeum-render-djvu/ Processo isolado que desenha DjVu
  musaeum-fs/          File IDs de 128 bits, listagem sem abrir arquivos, leitura que recusa
                       placeholders de nuvem, watcher que avisa quando perde eventos
  musaeum-flow/        Normalizador dos livros fluidos e motor de posição (CFI e Locator), todo
                       em Rust seguro: DOM, XML e HTML5, sanitizador, CSS, corte em partes,
                       leitores de EPUB, MOBI, KF8, FB2, TXT e Markdown
  musaeum-ingest/      Processo isolado que lê identificadores e metadados dos livros, o que o
                       catálogo de um PDF pede (`/PageLayout`, `/Direction`), e normaliza os
                       livros fluidos
  musaeum-library/     SQLite, reconhecimento de arquivos, indexador, diário de operações,
                       posição de leitura, geometria das páginas e o cache dos livros normalizados
apps/cli/              O binário `musaeum`: a biblioteca pela linha de comando
apps/desktop/
  src-tauri/           Ponte Tauri: protocolos tile:// e book://, buffers do WebView2, harness
  src/                 Thread principal e compositor WebGL2 em Web Worker: modos de leitura,
                       páginas duplas, física da passagem de página e cache de tiles
  src/reader/          Leitor fluido, numa origem própria, com o foliate-js atrás de um adaptador
vendor/
  foliate-js/          Parte do foliate-js, fixada em um commit, com uma alteração documentada
  libarchive/          libarchive 3.8.11, o lado de leitura, com origem e config.h documentados
  djvulibre/           DjVuLibre 3.5.30, a biblioteca libdjvu
  third-party.json     Licenças do que entra no app sem passar por um gerenciador de pacotes
tools/
  fetch-pdfium.ps1     Baixa a build fixada do PDFium e confere o SHA-256
  gen-fixtures/        Gera os PDFs, quadrinhos, DjVu e EPUBs de teste, um EPUB hostil e um mesmo
                       livro em EPUB 3, EPUB 2, MOBI, AZW3, FB2, TXT e Markdown
  pool-bench/          Mede o pool de render sem WebView
  library-bench/       Mede a biblioteca com 100 mil arquivos reais, tecla por tecla
  corpus-check/        Valida a biblioteca, o pipeline e o normalizador num acervo real, só lendo
  cfi-browser-check.mjs  Resolve no Chrome, com o foliate-js, os CFIs que o motor escreve
  visual-check/        Compara a janela com páginas desenhadas inteiras, e as cores do PDFium
  e2e-reader.mjs       Testa a janela real: modos, atalhos, posição salva e comparação visual
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

Abrir um documento de forma interativa:

```powershell
.\target\release\musaeum-desktop.exe --document fixtures\spreads.pdf
.\target\release\musaeum-desktop.exe --document fixtures\comic.cbz
.\target\release\musaeum-desktop.exe --document fixtures\book.djvu
.\target\release\musaeum-desktop.exe --document D:\Livros\um-livro.pdf --library D:\MinhaBiblioteca
.\target\release\musaeum-desktop.exe --epub fixtures\large.epub
```

`--document` abre PDF, CBZ, CBR ou DjVu; o formato vem dos primeiros bytes do arquivo, não do nome, e
`--pdf` continua valendo como o mesmo comando. A barra de leitura no alto troca entre paginado e
contínuo, liga e desliga a animação da passagem de página (que não existe no contínuo), escolhe páginas
duplas e muda o zoom. `C` alterna entre paginado e contínuo e `A` liga e desliga a animação. As
setas, Page Up/Down, Espaço, Home e End navegam; Ctrl com +, - e 0 mudam o zoom, e Ctrl com a roda ou o
gesto de pinça amplia em torno do ponteiro. Com `--library`, ou com a biblioteca padrão se ela já existir, a
posição, o modo, o zoom e as páginas duplas de um livro da biblioteca são guardados e voltam na próxima
abertura. Abrir um arquivo nunca cria uma biblioteca. `--tile-budget-mb N` limita a memória de GPU dos
tiles. No EPUB, as setas passam as páginas.

Rodar as medições. Elas abrem janelas em primeiro plano, uma por vez, e escrevem os relatórios
em `bench-results/`. Rode com a máquina livre: outro programa usando a GPU muda os resultados.

```powershell
node tools\run-bench.mjs
node tools\run-bench.mjs --scenario quick --uncapped --stress 0
.\target\release\musaeum-desktop.exe --pdf UM.pdf --bench zoom.json --scenario zoom
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

## O leitor de layout fixo

- **Escala exata.** Os tiles são rasterizados na escala da tela, em passos de 1/4096 pixel por ponto,
  e não em potências de 2. Durante um zoom, o compositor mostra o que já tem, reamostrado; os tiles da
  escala final são pedidos durante a animação e chegam antes de ela acabar.
- **Geometria sem rasterizar.** O tamanho de cada página e o `/PageLayout` vêm de um worker, sem desenhar
  nada, e ficam no `cache.db` pelos bytes do arquivo. Na próxima abertura, ninguém mede de novo.
- **Páginas duplas.** Mantêm a paridade impressa com espaços vazios, deixam a paisagem sozinha e
  respeitam `/PageLayout` e a leitura da direita para a esquerda.
- **Workers.** Um worker que cai ou é encerrado é substituído em dezenas de milissegundos, e a leitura
  segue. Um tile que derruba workers duas vezes deixa de ser pedido.
- **Três formatos, um pipeline.** PDF vai para o PDFium, quadrinhos para a libarchive e decodificadores
  de imagem em Rust, DjVu para a DjVuLibre. Cada um num worker isolado que fala o mesmo protocolo, então
  tiles, geometria, páginas duplas, modos e recuperação são os mesmos. Os workers de quadrinhos e DjVu
  ligam tudo estaticamente e rodam no sandbox mais estrito, sem win32k e só com DLLs da Microsoft.
- **Do que a página é feita.** Em segundo plano, cada página é classificada como texto, escaneada (uma
  imagem da página, sem camada de texto) ou híbrida (a imagem com o texto de um OCR por cima). A classe,
  as medidas que a decidiram e o resumo do documento ficam em `page_geometry` e `document_geometry`,
  no `cache.db`.
- **Miniaturas.** Na menor prioridade do agendador, nunca em todos os workers e canceladas assim que um
  tile visível precisa do worker, cada página ganha uma miniatura de até 256 px em
  `cache\thumbnails\v1\ab\<hash>\<página>.jpg`, pelo hash BLAKE3 do arquivo (`ab` são os dois
  primeiros dígitos dele).

## Validar num acervo real

```powershell
cargo build --release -p corpus-check -p musaeum-render-pdf -p musaeum-library --bins
.\target\release\corpus-check.exe --corpus D:\Livros --work D:\Temp\verificacao --sample 12
```

Indexa a árvore inteira, mede e classifica todas as páginas de todos os PDFs, desenha as miniaturas de
uma amostra, renderiza essa amostra, cancela renders pesados no meio e mata workers durante a leitura. O acervo só é lido: a ferramenta lista
tamanho, datas e atributos de tudo antes e depois e falha se algo mudar. Banco, cache e relatório ficam
em `--work`, que precisa estar fora do acervo. Tudo roda abaixo da prioridade normal.

Com `--flow-only`, ela faz outra coisa: normaliza duas vezes cada livro fluido no processo isolado,
guarda o resultado no cache e confere cada posição de cada documento, como CFI e Locator, e cada CFI de
EPUB contra o documento original:

```powershell
.\target\release\corpus-check.exe --corpus D:\Livros --work D:\Temp\fluido --flow-only
node tools\cfi-browser-check.mjs --cache D:\Temp\fluido\cache
.\target\release\musaeum.exe normalize livro.mobi D:\Temp\livro-normalizado.epub
```

## A janela real e a comparação visual

```powershell
cd apps\desktop; npm run build:app; cd ..\..
cargo build --release -p musaeum-cli -p visual-check -p musaeum-library --bins
node tools\e2e-reader.mjs --corpus-sample amostra.txt
```

Conduz a janela real pelo protocolo de depuração do próprio WebView2: troca de modo pela barra e pelos
atalhos, com e sem animação, confere que o Locator (página e ponto u, v) não se mexe, fecha, reabre e
confere que o `library.db` devolveu exatamente o que foi salvo. Depois compara páginas da janela com as
mesmas páginas desenhadas inteiras pela biblioteca de cada formato, e as cores do PDFium com as
referências. Cliques e teclas entram pela página, sem mover o mouse nem mandar teclas para a janela que
estiver em foco, mas a janela aparece: a ferramenta só começa se ninguém usou o computador no último
minuto. `amostra.txt` lista, por linha, `caminho|página,página` de livros de um acervo, que só são
lidos.

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

Todo arquivo de livro é entrada hostil, então cada worker é tratado como se já estivesse comprometido. Ele roda
com um token restrito sem nenhum grupo utilizável, em nível de integridade baixo, com mitigações
de processo, em um job object próprio e em uma estação de janelas separada. O worker carrega o
PDFium, entra em lockdown e só depois lê qualquer mensagem do host. Daí em diante, só tem o que
o host duplicou para ele: um handle somente leitura do documento e a memória compartilhada.

`tests/sandbox.rs` prova isso tentando: `sandbox-probe` se comporta como um worker comprometido,
e cada tentativa negada dentro do sandbox precisa dar certo fora dele.

Os workers de quadrinhos e DjVu não carregam DLL nenhuma de terceiros, então rodam com uma política
ainda mais estrita: sem win32k e sem DLLs que a Microsoft não assinou. O do PDF não pode, porque o
`pdfium.dll` atual usa o user32.

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

`tools/test-epub-security.mjs` testa cada barreira sozinha e, como controle, sem nenhuma. A Fase 3
acrescenta uma quarta barreira, antes das outras: o normalizador tira scripts, eventos e referências
que carregam de fora, sem mudar a posição de nada. Com `--normalized`, o teste abre o livro hostil
normalizado sem nenhuma outra barreira.

## Testes

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd apps\desktop; npm test; npx tsc --noEmit; cd ..\..
node --test tools\lib\spdx.test.mjs
node tools\check-licenses.mjs
```

Os testes de integração em `crates/musaeum-render-pdf/tests/` usam processos reais, memória
compartilhada real e o PDFium real, então exigem o `fetch-pdfium.ps1` antes. Os de
`musaeum-render-raster` e `musaeum-render-djvu` fazem o mesmo com a libarchive e a DjVuLibre, que
são compiladas do fonte vendorizado pelo compilador C++ do Visual Studio.

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
