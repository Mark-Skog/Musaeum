# Musaeum

Leitor e gerenciador de livros digitais, livre e de código aberto (GPL-3.0-or-later).

O projeto está na **Fase 3 do Plano Diretor**, o layout fluido, e chegou ao **marco Alpha**: uma
biblioteca de uso diário com o leitor integrado. O que existe hoje: a biblioteca, que segue as pastas
escolhidas e lista os livros em capas ou em lista, com busca instantânea e filtro por pasta; o leitor
fluido para EPUB 2 e 3, MOBI, AZW3, FB2, TXT e Markdown, que lê cada livro normalizado num processo isolado
e mostra em páginas, com a virada animada, ou numa rolagem contínua própria, com fonte, tamanho,
entrelinha, margens e tema; o leitor de layout fixo da Fase 2 para PDF, quadrinhos (CBZ e CBR) e DjVu; a
mesma barra, o mesmo sumário e os mesmos atalhos para os dois; e a posição de cada livro guardada na
biblioteca, exata ao fechar e reabrir. Por baixo, o normalizador e o motor de posição (EPUB CFI e
Locator) da Fase 3, o pipeline de tiles da Fase 2 e a biblioteca da Fase 1, com SQLite, identidade de
arquivos, indexador, diário de operações em disco e uma CLI.

As decisões e os números de cada fase estão em
[`docs/adr/0001-fase-0.md`](docs/adr/0001-fase-0.md),
[`docs/adr/0002-fase-1.md`](docs/adr/0002-fase-1.md),
[`docs/adr/0003-fase-2-parte-1.md`](docs/adr/0003-fase-2-parte-1.md),
[`docs/adr/0004-fase-2-parte-2.md`](docs/adr/0004-fase-2-parte-2.md),
[`docs/adr/0005-fase-3-parte-1.md`](docs/adr/0005-fase-3-parte-1.md) e
[`docs/adr/0006-fase-3-parte-2.md`](docs/adr/0006-fase-3-parte-2.md).

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
  src-tauri/           Ponte Tauri: protocolos tile://, book:// e thumb://, a biblioteca do app
                       e o indexador, capas, livros normalizados, buffers do WebView2, harness
  src/app/             A interface: biblioteca e leitor, em React com React Aria
  src/fixed/           O leitor de layout fixo na thread principal
  src/                 Compositor WebGL2 em Web Worker: modos de leitura, páginas duplas, física
                       da passagem de página e cache de tiles
  src/reader/          Leitor fluido, numa origem própria: a vista paginada (foliate-js atrás de
                       um adaptador), a rolagem contínua própria, a âncora e a tipografia
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
  e2e-alpha.mjs        Testa o Alpha na janela real: biblioteca, leitor e a posição em cada formato
  reader-check.mjs     Testa o leitor fluido sem janela, num Chrome headless, em livros normalizados
  lib/cdp.mjs          O que os testes de janela usam para falar com o WebView2 pelo CDP
  lib/seam-oracle.mjs  Onde o texto depois de um corte deve cair, pelo documento inteiro
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

Abrir o app:

```powershell
.\target\release\musaeum-desktop.exe
.\target\release\musaeum-desktop.exe --library D:\MinhaBiblioteca
```

Sem biblioteca, o app pede a pasta dos livros, ou a pasta de uma biblioteca feita antes; ele cria a
biblioteca em `%LOCALAPPDATA%\Musaeum`, ou na pasta que `--library` der, e nunca escreve na pasta dos
livros. Um clique ou Enter abre o livro; `← Biblioteca` (ou Alt+←) volta. No leitor, `C` alterna entre
paginado e contínuo e `A` liga e desliga a animação da virada, nos dois tipos de livro; `Aa` muda a
tipografia de um livro fluido e `Sumário` mostra o sumário (o de um PDF vem dos seus marcadores).

Para anotar, selecione um trecho: arrastando, ou com dois cliques numa palavra, três numa linha e, num
livro fixo, quatro num parágrafo; a seleção atravessa páginas e, no contínuo, as partes de um capítulo
longo. Uma barra aparece sobre a seleção, com cinco cores de realce, sublinhar, nota (em Markdown, com
pré-visualização) e copiar; um clique numa marca abre a mesma barra, com remover (e "Desfazer").
`Anotações` abre o Inspetor, que lista as do livro pela posição, filtra por cor, tipo e órfãs, busca nas
citações e notas (sem se importar com acentos) e leva ao trecho com um clique. As anotações ficam no
`library.db` e só existem para livros da biblioteca; uma que perdeu o seu trecho numa nova edição do
arquivo continua listada como órfã, com a citação e a nota.

Abrir um documento direto, como nas fases anteriores e nas medições:

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

O Alpha tem o seu teste de janela, que usa o app inteiro, da biblioteca ao leitor:

```powershell
node tools\e2e-alpha.mjs --phase fixtures
node tools\e2e-alpha.mjs --phase corpus --corpus D:\Livros
```

Com as fixtures, cria uma biblioteca numa pasta temporária com um livro em cada formato, busca, filtra
por pasta, abre cada livro com um clique e volta. Em cada formato fluido, vai ao meio do livro e muda
fonte, entrelinha, margens, família, tema e janela, e alterna paginado e contínuo, conferindo depois de
cada mudança que a âncora é o mesmo caractere e que está na página mostrada (paginado) ou a até uma linha
do topo (contínuo); depois fecha e reabre o livro nos dois modos e confere a posição exata. Com um
acervo, abre cada EPUB e cada PDF a partir da biblioteca e confere que o acervo não mudou. A ferramenta
para se alguém usar o computador no meio.

O leitor fluido também se testa sem janela, num Chrome headless com perfil temporário, nos livros que
o `corpus-check --flow-only` normalizou na pasta de trabalho:

```powershell
.\target\release\corpus-check.exe --corpus D:\Livros --work D:\Temp\fluido --flow-only
node tools\reader-check.mjs --cache D:\Temp\fluido\cache
```

Serve o `reader.html` construído com a política do app, roda o mesmo portão de posição da janela,
reabre cada livro nos dois modos e confere cada emenda entre partes contra o documento inteiro, refeito
das partes. Depois arrasta o mouse sobre um trecho na tela, confere que a seleção é exatamente aquele
texto, desenha uma marca e confere, a cada mudança de fonte, margens, modo e tema, que ela continua sobre
o seu texto; onde o livro tem um corte, arrasta de uma parte para a outra. Não abre janela nem usa input do
sistema, e pode rodar com o computador em uso.

As anotações têm o seu teste de janela:

```powershell
node tools\e2e-annotations.mjs --phase fixtures
node tools\e2e-annotations.mjs --phase corpus --corpus D:\Livros
```

Com as fixtures, escreve um PDF e um EPUB em duas edições (`tools/lib/test-books.mjs`) e usa o DjVu das
fixtures: realça com um arrasto, sublinha com dois cliques, anota com três, marca através da quebra de
página e do corte de um capítulo longo, confere na própria captura da página que o realce está desenhado
sob a linha, muda zoom, fonte, margens e modo, busca no Inspetor e salta; depois fecha e reabre, renomeia
os arquivos, move-os de pasta e grava por cima a segunda edição (*safe save*), e confere que nenhuma
anotação se perdeu: as de trechos mantidos exatas, a da frase editada aproximada, a do trecho removido
órfã. Com um acervo, anota alguns EPUBs e PDFs e confere que o acervo não mudou.

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
