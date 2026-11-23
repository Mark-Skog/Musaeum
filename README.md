# Musaeum

Leitor e gerenciador de livros digitais, livre e de código aberto (GPL-3.0-or-later).

O projeto chegou ao **marco Beta** do Plano Diretor (o fim da Fase 6, a organização da biblioteca),
depois do **marco Alpha** (uma biblioteca de uso diário com o leitor integrado), das anotações de texto
da Fase 4 e das anotações sobre livros escaneados da Fase 5. O que existe hoje: a biblioteca, que segue
as pastas escolhidas e lista os livros em capas ou em lista, com busca instantânea nos títulos ou no
texto dos livros (com o trecho achado, a página ou a seção, e o livro aberto ali), e os filtra por pasta,
por uma árvore de categorias e subcategorias, por etiquetas, por coleções feitas à mão (na ordem que a
pessoa dá) e por coleções inteligentes (uma regra, como "autor contém Sagan e ano entre 1980 e 1999"); um
painel de detalhes que edita o título, as pessoas e os seus papéis, a série, o ano, o idioma, os
identificadores, a avaliação, o estado de leitura, a sinopse e campos personalizados, de um livro ou de
vários escolhidos de uma vez, sem que uma releitura do arquivo escreva por cima do que a pessoa editou;
a revisão dos livros que podem ser um só, e a fusão deles sem perder anotações nem categorias; a
importação de uma biblioteca do Calibre, só lida; o leitor
fluido para EPUB 2 e 3, MOBI, AZW3, FB2, TXT e Markdown, que lê cada livro normalizado num processo isolado
e mostra em páginas, com a virada animada, ou numa rolagem contínua própria, com fonte, tamanho,
entrelinha, margens e tema; o leitor de layout fixo da Fase 2 para PDF, quadrinhos (CBZ e CBR) e DjVu; a
mesma barra, o mesmo sumário e os mesmos atalhos para os dois; e a posição de cada livro guardada na
biblioteca, exata ao fechar e reabrir; realces, sublinhados e notas em Markdown, em livros fixos com
texto e em fluidos, ancorados de três jeitos e achados de novo depois de renomear, mover ou editar o
arquivo, com o Inspetor de Anotações e busca; e, nos livros escaneados, as linhas de texto achadas na
imagem da página, retas, curvas ou giradas a 45° e 90°, para selecionar por elas, ou marcar uma caixa
ou um traço à mão, e o texto sob cada marca lido por OCR (Tesseract, num processo isolado), para
copiar e buscar como num livro digital. Por baixo, o normalizador e o motor de posição (EPUB CFI e
Locator) da Fase 3, o pipeline de tiles da Fase 2 e a biblioteca da Fase 1, com SQLite, identidade de
arquivos, indexador, diário de operações em disco e uma CLI.

As decisões e os números de cada fase estão em
[`docs/adr/0001-fase-0.md`](docs/adr/0001-fase-0.md),
[`docs/adr/0002-fase-1.md`](docs/adr/0002-fase-1.md),
[`docs/adr/0003-fase-2-parte-1.md`](docs/adr/0003-fase-2-parte-1.md),
[`docs/adr/0004-fase-2-parte-2.md`](docs/adr/0004-fase-2-parte-2.md),
[`docs/adr/0005-fase-3-parte-1.md`](docs/adr/0005-fase-3-parte-1.md),
[`docs/adr/0006-fase-3-parte-2.md`](docs/adr/0006-fase-3-parte-2.md),
[`docs/adr/0007-fase-4.md`](docs/adr/0007-fase-4.md),
[`docs/adr/0008-fase-5-parte-1.md`](docs/adr/0008-fase-5-parte-1.md),
[`docs/adr/0009-fase-5-parte-2.md`](docs/adr/0009-fase-5-parte-2.md),
[`docs/adr/0010-fase-6-parte-1.md`](docs/adr/0010-fase-6-parte-1.md) e
[`docs/adr/0011-fase-6-parte-2.md`](docs/adr/0011-fase-6-parte-2.md).

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
                       posição de leitura, geometria das páginas, anotações e a re-ancoragem, o
                       cache dos livros normalizados e os modelos de linhas
  musaeum-vision/      As linhas de texto na imagem de uma página escaneada: Sauvola, componentes,
                       inclinação, colunas por faixas, linhas de base como polilinhas, texto girado,
                       a máscara de fotografias, gráficos e tabelas, hash perceptual; e um corpus
                       rotulado para medi-lo
  musaeum-tesseract/   Tesseract e Leptonica compilados do fonte, atrás de uma ponte em C++
  musaeum-ocr-worker/  Processo isolado que lê com o Tesseract uma linha endireitada de cada vez
  musaeum-ocr/         A conversa com esse worker, o lado do host e a geometria das linhas
                       endireitadas ao longo da linha de base
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
  tesseract/           Tesseract 5.5.4, Leptonica 1.87.0 e os modelos por e eng (baixados pelo
                       fetch-ocr.ps1; só o README está no repositório)
  third-party.json     Licenças do que entra no app sem passar por um gerenciador de pacotes
tools/
  fetch-pdfium.ps1     Baixa a build fixada do PDFium e confere o SHA-256
  fetch-ocr.ps1        Baixa o Tesseract, o Leptonica e os modelos fixados e confere o SHA-256
  gen-fixtures/        Gera os PDFs, quadrinhos, DjVu e EPUBs de teste, um EPUB hostil e um mesmo
                       livro em EPUB 3, EPUB 2, MOBI, AZW3, FB2, TXT e Markdown
  pool-bench/          Mede o pool de render sem WebView
  library-bench/       Mede a biblioteca com 100 mil arquivos reais, tecla por tecla
  corpus-check/        Valida a biblioteca, o pipeline e o normalizador num acervo real, só lendo
  cfi-browser-check.mjs  Resolve no Chrome, com o foliate-js, os CFIs que o motor escreve
  visual-check/        Compara a janela com páginas desenhadas inteiras, e as cores do PDFium
  e2e-reader.mjs       Testa a janela real: modos, atalhos, posição salva e comparação visual
  e2e-alpha.mjs        Testa o Alpha na janela real: biblioteca, leitor e a posição em cada formato
  e2e-taxonomy.mjs     Testa na janela real a organização da biblioteca e o painel de detalhes
  e2e-beta.mjs         Testa na janela real a Beta: vários livros, revisão e fusão, texto, Calibre
  reader-check.mjs     Testa o leitor fluido sem janela, num Chrome headless, em livros normalizados
  lib/cdp.mjs          O que os testes de janela usam para falar com o WebView2 pelo CDP
  lib/clipboard.mjs    A área de transferência nos testes de janela, guardada antes e reposta depois
  lib/seam-oracle.mjs  Onde o texto depois de um corte deve cair, pelo documento inteiro
  lib/calibre-library.mjs  Uma biblioteca do Calibre sintética, do esquema de lib/calibre-schema.sql
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
powershell -ExecutionPolicy Bypass -File tools\fetch-ocr.ps1

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
livros. Um clique escolhe um livro e mostra os seus detalhes; o duplo clique ou Enter o abre, e
`← Biblioteca` (ou Alt+←) volta. No leitor, `C` alterna entre
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

## A organização da biblioteca

- **O lado da biblioteca.** *Todos os livros*, as *Pastas* do disco, as *Categorias* (uma árvore de até
  16 níveis), as *Etiquetas*, as *Coleções* e as *Coleções inteligentes*. Escolher um deles mostra os
  livros dele, com a busca por cima; uma categoria mostra também os das subcategorias, e uma coleção, na
  ordem dela. Cada item tem um menu "⋯" (renomear, mover, subir, descer, excluir) e cada seção um "+".
  Mover uma categoria leva as subcategorias junto e nunca a põe dentro de si mesma.
- **Coleções inteligentes.** Uma regra por campos e condições (título, autor, série, idioma,
  identificador, ano, avaliação, lido %, estado, formato, datas, etiqueta, categoria, coleção, pasta e os
  campos personalizados), com "todas" ou "qualquer", sem diferença de maiúsculas e acentos. O diálogo
  diz quantos livros a regra acha enquanto ela é escrita.
- **Detalhes.** Um clique num livro o seleciona e abre o painel à direita; o duplo clique ou Enter o
  abre, e Esc guarda o painel. O painel edita a obra e diz de onde veio cada campo ("do arquivo", "editado"). Só *Salvar*
  grava, e só o que mudou; o que a pessoa editou é dela, e nenhuma leitura do arquivo depois (uma
  reimportação, uma nova edição do arquivo) escreve por cima. Um ISBN é conferido pelo dígito
  verificador.
- **Campos personalizados.** Definidos uma vez (*Campos personalizados…*, no pé do lado), de um tipo:
  texto, número, data, sim/não, lista de opções ou endereço web; cada livro tem o seu valor.
- **Vários livros de uma vez.** Ctrl+clique acrescenta ou tira um livro da escolha, Shift+clique
  escolhe os do meio, Ctrl+A todos os da lista (também os ainda não carregados). Com dois ou mais, o
  painel edita todos: pessoas, série, ano, idioma, avaliação, estado, e categorias, etiquetas e coleções
  acrescentadas ou tiradas de todos. Um campo em que eles diferem mostra "(vários)"; *Salvar em N
  livros* grava tudo de uma vez, ou nada, e só os campos tocados.
- **Revisar.** No lado, com o número do que espera: arquivos que podem ser o mesmo e obras que podem ser
  um só livro (o mesmo identificador, o mesmo título do mesmo autor). Cada par se aceita ou se recusa, e
  recusado não volta. *Fundir* escolhe a obra que fica e traz para ela os arquivos das outras, cada um
  com as suas anotações, a posição de leitura e o histórico, e as categorias, etiquetas e coleções.
- **No texto.** Com algo digitado na busca, *Títulos / No texto*. No texto, cada livro aparece com as
  páginas ou seções em que estão todas as palavras (ou a "frase entre aspas"), um trecho com o achado
  realçado, e um clique abre o livro ali. O texto vem da camada de texto dos PDFs e DjVus, das linhas
  já lidas por OCR nos escaneados e do texto dos livros fluidos, lido em segundo plano e devagar, mais
  devagar ainda com um livro aberto. Ao voltar do livro, a busca está como foi deixada.
- **Do Calibre.** *Importar do Calibre…* escolhe a pasta da biblioteca (a do `metadata.db`), mostra o
  que ela tem e a traz: cada livro com os seus formatos numa obra, autores, série e número, etiquetas,
  avaliação, identificadores, comentários e colunas personalizadas como campos. A pasta é só lida, nem o
  `metadata.db` muda, e passa a ser acompanhada como as outras; um arquivo com os mesmos bytes de um que
  a biblioteca já tem é o mesmo livro. O que veio do Calibre aparece como "do Calibre" no painel, e uma
  releitura do arquivo não escreve por cima; o que a pessoa editar, nem o Calibre.

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
- **Livros escaneados.** Nas páginas escaneadas e híbridas, o worker desenha em segundo plano uma
  imagem de 1500 px de altura, e o processo principal acha as linhas de texto dela (em ≈ 20 ms) e guarda
  o modelo no `cache.db`. Na barra do leitor, *Linhas* seleciona da palavra onde o arrasto começa até a
  palavra onde termina, seguindo a curva das linhas perto da lombada (*Ajustar às palavras* desliga o
  ajuste); *Caixa* marca um retângulo translúcido; *Traço*, à mão livre. Cada marca é uma anotação.
  O detector acha também blocos de texto girados (legendas a 90° ao lado de uma figura, rótulos a 45°
  de um gráfico, linhas tortas) e deixa de fora fotografias, retículas, gráficos e as grades de
  tabelas, para que nem o *snap* nem o OCR peguem algo que não é texto. Uma página impressa de lado
  (uma tabela, a última página de um manual) é lida girada um quarto de volta, como uma página comum;
  traços de lápis, sublinhados e rabiscos na margem não desviam as linhas; e um arrasto segue só as
  linhas que correm no mesmo sentido da primeira, sem misturar o texto a 90° com uma marca a 0°.
- **O texto de um escaneado.** Ao desenhar uma marca com *Linhas*, *Caixa* ou *Traço*, o texto sob ela
  é lido da imagem pelo OCR, linha a linha, cada linha endireitada ao longo da sua base e lida pelo
  Tesseract num processo isolado (`musaeum-ocr.exe`, no mesmo sandbox dos workers). Numa página
  híbrida, a camada de texto do arquivo só entra quando o OCR não tem nada ali; a ferramenta *Texto*
  seleciona os caracteres dessa camada, como num livro digital. *Copiar*
  e Ctrl+C põem na área de transferência as linhas lidas, como numa seleção de um livro digital; o que
  cada linha diz fica no `cache.db` (`ocr_line`), e não é lido de novo. A anotação recebe o texto como
  citação em segundo plano, uma vez, e a busca do Inspetor passa a achá-la.
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

Com `--lines`, ela mede o detector de linhas da Fase 5 numa amostra das páginas escaneadas e híbridas,
também só lendo o acervo; `--dump PASTA` desenha as linhas achadas (vermelho), as do OCR (verde) e as
faixas e colunas (laranja) sobre cada página. Com `--lines-synthetic`, mede o detector no corpus
rotulado, que não precisa de acervo:

```powershell
.\target\release\corpus-check.exe --lines --corpus D:\Livros --work D:\Temp\linhas --sample 60 --out linhas.json
.\target\release\corpus-check.exe --lines-synthetic --pages 30 --out sintetico.json
```

Com `--inspect`, olha páginas escolhidas de um livro do acervo, numeradas como o leitor as mostra: o
detector acha as linhas delas, `--ocr` lê cada uma, e a imagem de cada página fica na pasta de trabalho
(PGM, que o exemplo `dump` do `musaeum-vision` relê, e JPEG com as linhas desenhadas). Também só lê:

```powershell
.\target\release\corpus-check.exe --inspect "D:\Livros\Manual.pdf" --at 14+79+100-120 --corpus D:\Livros --work D:\Temp\paginas --ocr --out paginas.json
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
por pasta, abre cada livro (um clique o escolhe, Enter o abre) e volta. Em cada formato fluido, vai ao meio do livro e muda
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

A organização da biblioteca e a Beta têm os seus:

```powershell
node tools\e2e-taxonomy.mjs
node tools\e2e-beta.mjs --phase synthetic
node tools\e2e-beta.mjs --phase real --corpus D:\Livros
```

O da Beta, com livros escritos para ele, escolhe vários com Ctrl+clique, Shift+clique e Ctrl+A e edita
dois de uma vez; busca uma palavra no texto e confere que o clique abre o PDF na página dela e o EPUB no
lugar exato, onde faz uma nota; funde duas obras com o mesmo ISBN e confere a nota, a etiqueta e a série
na que ficou, e recusa um par que não é o mesmo livro; importa uma biblioteca do Calibre sintética
(`tools/lib/calibre-library.mjs`, com colunas personalizadas, séries, etiquetas, um livro com os mesmos
bytes de um que já está na biblioteca e um sem arquivo) e confere que nenhum byte dela mudou; fecha,
reabre e confere tudo. Com um acervo, lista-o, busca pelo título e no texto, abre um achado e confere
que o acervo não mudou.

As anotações têm o seu teste de janela:

```powershell
node tools\e2e-annotations.mjs --phase fixtures
node tools\e2e-annotations.mjs --phase corpus --corpus D:\Livros
node tools\e2e-annotations.mjs --phase lines
node tools\e2e-annotations.mjs --phase ocr
```

Com `--phase lines`, escreve um livro escaneado sem camada de texto e confere, na janela, as linhas que
o segundo plano acha na imagem contra as desenhadas, a seleção por linhas com e sem o ajuste às
palavras, a caixa e o traço, cada um desenhado na tela e achado de novo ao reabrir.

Com `--phase real` e os relatórios de `corpus-check --inspect --ocr` (`--real a.json+b.json`), abre as
pastas de dois livros do acervo numa biblioteca nova, só lendo, e confere na janela, nas páginas reais
em que a ferramenta Linhas falhava à mão, o que um arrasto copia: um sumário com pontilhado e uma linha
curta em itálico, uma linha sublinhada a lápis com rabiscos na margem e uma página impressa de lado.

Com `--phase ocr`, põe na mesma biblioteca um livro com camada de texto (`gen-fixtures --only ocr`) e o
seu gêmeo escaneado (`visual-check scan`) e confere, pela área de transferência do sistema, que copiar
três linhas do gêmeo com a ferramenta Linhas dá o mesmo que copiá-las do original, palavra por palavra e
linha por linha; que uma caixa copia as linhas dentro dela; que o texto a 90°, 45° e −30° sai como se
lê; que uma caixa sobre a fotografia não copia nada e um arrasto nela não seleciona nada; que a marca no
gêmeo recebe a citação do original, é achada pela busca sem acentos e a mantém ao reabrir; e que nenhum
worker de OCR fica rodando depois. A área de transferência é guardada antes e reposta depois.

O detector e o OCR também se medem sem janela, no mesmo livro e em três gêmeos (limpo, girado 1,5° e
curvado na lombada):

```powershell
.\target\release\corpus-check.exe --ocr-book --work D:\Temp\ocr --out ocr.json
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
