# ADR 0013: decisões da Fase 7, Parte 2, o intercâmbio em JSON-LD e os relatórios de estudo

| | |
|---|---|
| **Data** | 2026-11-23 |
| **Situação** | Aceita na auditoria de 2026-11-23, sem emendas (seção 9) |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo, WebView2 157 |
| **Escopo** | Levar e trazer as anotações de qualquer formato como Web Annotations do W3C em JSON-LD, sem perder nada, de um livro, de várias obras ou da biblioteca toda; trazê-las de volta sem duplicar, para o mesmo arquivo ou para outra edição da mesma obra, achadas de novo pela cascata das Fases 4 e 5; escrever relatórios de estudo em Markdown e em PDF; as ações no leitor e na biblioteca; testes e a passada na janela real |

## 1. Resumo

- **Exportar JSON-LD…** grava uma `AnnotationCollection` do modelo de Web Annotations do W3C (2017):
  cada anotação com o UUIDv7 (`urn:uuid:…`), as datas, a nota em `text/markdown`, a cor, o tipo, a
  citação e de onde ela veio, a posição no livro, o estado da âncora, o nome do PDF de onde veio, e um alvo
  por trecho, com **todos** os seletores que a biblioteca guarda, cada um com os seus dados, a prioridade,
  a versão e a revisão em que foi calculado. O alvo aponta para os bytes exatos do arquivo
  (`urn:blake3:…`) e descreve a obra: título, pessoas e papéis, série, ano, idioma, identificadores.
- **Importar JSON-LD…** confere o arquivo inteiro antes de gravar qualquer coisa e grava tudo numa
  transação só. Uma anotação que a biblioteca já tem, pelo UUID, não vem de novo. Cada uma volta para o
  livro com os mesmos bytes; se ele não está, para um livro da mesma obra, onde é achada de novo na hora,
  exata, aproximada ou órfã, e nunca perdida.
- **Relatório em Markdown…** e **Relatório em PDF…** escrevem um caderno de estudo de um livro ou de
  várias obras: o cabeçalho com o que se sabe da obra, as anotações na ordem de leitura, sob os capítulos
  do sumário, as citações e as notas, e as não encontradas ao final.

As ações estão no painel *Anotações* do leitor (qualquer formato), nos detalhes de uma obra e de vários
livros escolhidos na biblioteca, e no menu *Anotações* da barra da biblioteca (a biblioteca toda). Nada é
gravado dentro de uma pasta da biblioteca.

## 2. Entregas

| Peça | Onde |
|---|---|
| O modelo do W3C, a escrita e o leitor estrito, o casamento com a obra e a gravação idempotente | `crates/musaeum-library/src/exchange.rs` |
| Os relatórios: o modelo, a ordem e os capítulos | `crates/musaeum-report/src/{lib,order}.rs` |
| O Markdown das notas, lido como o leitor o lê | `crates/musaeum-report/src/notes.rs` e os casos comuns `apps/desktop/src/app/reader/annotations/notes-cases.json` |
| O relatório em Markdown e o seu escape | `crates/musaeum-report/src/markdown.rs` |
| O PDF: composição, paginação e o arquivo | `crates/musaeum-report/src/pdf.rs` |
| TrueType: cmap, métricas e subconjuntos renumerados | `crates/musaeum-report/src/font.rs` |
| Acentos combinantes compostos, caracteres invisíveis | `crates/musaeum-report/src/text.rs` |
| Datas sem relógio nem fuso | `crates/musaeum-report/src/time.rs` |
| As fontes, fixadas | `tools/fetch-fonts.ps1` → `vendor/fonts/` (fora do repositório) |
| Os comandos, a escrita atômica, a recusa das pastas da biblioteca, a re-ancoragem fora do leitor, a conferência do PDF | `apps/desktop/src-tauri/src/exchange.rs` |
| A interface | `apps/desktop/src/app/{exchange.ts,ExchangeSection.tsx}`, `AnnotationUi.tsx`, `ReaderView.tsx`, `library/{Inspector,BatchPanel,LibraryView}.tsx` |
| As correções achadas no caminho (seção 7): o OCR no índice do texto, os números lidos com todos os bits | `crates/musaeum-library/src/fulltext.rs`, `migrations/cache/0007_fulltext_ocr_again.sql`, `apps/desktop/src-tauri/src/ocr.rs`, `Cargo.toml` (`float_roundtrip`) |
| Testes | `musaeum-library/tests/exchange.rs`, os testes de unidade de `musaeum-report` e do host, `musaeum-render-pdf/tests/report.rs` (o PDF no PDFium), `musaeum-library/tests/reading.rs` (o OCR no índice), `app/exchange.test.ts`, `markdown.test.tsx`, `tools/e2e-exchange.mjs` |

## 3. Decisões

### D1. O arquivo é o modelo do W3C, e o que ele não diz vai num vocabulário próprio

Um arquivo é uma `AnnotationCollection` com uma `AnnotationPage` (`startIndex` 0, `total` igual ao
número de itens), o `generator` (`Software`, `Musaeum` e a versão) e `generated`. Uma anotação é uma
`Annotation` com `motivation` `highlighting` (realce e sublinhado) ou `commenting` (nota), `created` e
`modified` com os milissegundos, e o corpo, quando há nota, um `TextualBody` em `text/markdown` com
`purpose` `commenting`. Uma nota sobre o livro todo tem um alvo sem seletores.

Cada trecho é um alvo `SpecificResource`. A fonte é o arquivo exato, `urn:blake3:` e os 64 dígitos da
revisão em que os seletores foram calculados, do tipo `Text`, com o tipo de mídia, `dcterms:title` e
`dcterms:creator` (para outros leitores), e no vocabulário próprio as pessoas com os papéis, a série, o
ano, o idioma, os identificadores da obra, o `/ID[0]` do PDF, o `dc:identifier` e o ISBN do documento,
o layout e o formato. Os seletores são os do W3C quando dizem exatamente o que a biblioteca guarda:

| Na biblioteca | No arquivo |
|---|---|
| `text_quote` `{exact, prefix, suffix}` | `TextQuoteSelector` |
| `text_position` `{start, end}` | `TextPositionSelector`, com `musaeum:unit` `utf-16` (a biblioteca conta em unidades de UTF-16 do texto normalizado do documento; o W3C conta pontos de código) |
| `cfi` `{start, end}` | `RangeSelector` de dois `FragmentSelector` com `conformsTo` do EPUB CFI |
| `page_char_range`, `page_quads`, `page_region` | `musaeum:PageCharRangeSelector`, `musaeum:PageQuadsSelector`, `musaeum:PageRegionSelector`, com os dados como estão guardados em `musaeum:value` |

Um seletor do W3C cujos dados tivessem algo a mais iria como o da biblioteca, inteiro, e nada se perde.
Cada um leva a prioridade, a versão do que conta e a revisão (`urn:blake3:`) em que foi calculado. Um
trecho de uma página leva também um `FragmentSelector` do PDF (`page=N`), derivado, para outros leitores;
na leitura ele tem de dizer a mesma página. O vocabulário é `urn:musaeum:annotation:` (o projeto não tem
domínio, e o código já nomeia coisas `urn:musaeum:…`): um nome, nunca buscado. O `@context` é o do W3C,
pelo endereço, e um objeto com o prefixo e os termos cujo valor é JSON cru (`@json`): as listas ordenadas
de quads, caixas, traços, pessoas e identificadores, que o JSON-LD trataria como conjuntos.

### D2. O leitor é estrito, e nada é buscado

Antes de construir qualquer coisa: no máximo 256 MB, aninhamento de no máximo 16 níveis (contado nos
bytes, fora das strings). Depois, cada objeto só com as propriedades que esta versão escreve, cada uma uma
vez (uma chave repetida é recusada), com o tipo certo; o `@context` exatamente o que ela escreve (nem
`@import`, nem outro endereço; nenhum é buscado, e não há rede aqui); `musaeum:formatVersion` 1; o total
conferido; UUIDs na versão 7; datas `xsd:dateTime` válidas; fontes `urn:blake3:` com 64 dígitos
minúsculos; cada valor nos limites da biblioteca (nota de 1 MiB, citação de 256 KiB, seletor de 1 MiB,
4.096 trechos, 16 seletores por trecho, 500 mil anotações). Nada executa: são dados. Um erro em qualquer
anotação recusa o arquivo inteiro e nada é gravado.

### D3. A identidade é o UUID

Uma anotação que a biblioteca tem com o mesmo UUID, apagada ou não, em qualquer livro, fica como está: o
mesmo arquivo trazido duas vezes não acrescenta nada, e um backup antigo não desfaz uma edição nem traz de
volta uma anotação apagada. Uma com o nome de PDF (`/NM`) de uma anotação do mesmo livro também (o que a
Parte 1 já faz). As apagadas não são exportadas.

### D4. Para onde cada anotação vai

1. **Os mesmos bytes:** a revisão de um de seus `urn:blake3` é de um documento da biblioteca: vai para ele.
2. **A mesma obra:** um identificador em comum (os da obra, menos `other`; o ISBN, o `/ID[0]` do PDF ou
   o `dc:identifier` do documento) ou o mesmo título e o mesmo primeiro autor, comparados sem acentos nem
   maiúsculas, a regra da revisão de duplicatas da Fase 6. Vai para um documento dessa obra com o mesmo
   layout (fixo ou fluido), do mesmo formato se houver, o mais novo.
3. **O livro escolhido:** do leitor ou dos detalhes de uma obra, se o arquivo inteiro é de uma obra só e o
   layout combina, vai para o livro escolhido, e o relatório diz que veio de outra obra.
4. **Nenhum:** fica no arquivo, e o relatório diz quais obras a biblioteca não tem e quantas anotações
   esperam por cada uma.

Do leitor, só entra o que é do livro aberto; dos detalhes de uma obra, só o que é dela; do menu da
biblioteca, tudo o que tiver para onde ir. Um título sozinho não basta (diz pouco demais); como a
biblioteca não lê o título e o autor de dentro de um PDF, outra edição de um PDF casa pelo `/ID[0]`, que
uma edição salva por cima mantém, por um identificador, ou pelo que a pessoa escreveu nos detalhes.

### D5. Outra edição: achada de novo na hora

Uma anotação que vem dos mesmos bytes entra com tudo como estava: a revisão em que foi feita e a em que foi
achada são as da biblioteca com os mesmos bytes. Uma que vem de outros bytes entra feita sobre a revisão
mais nova do documento, sem revisão de onde foi achada, e os seletores sem revisão: a cascata das Fases 4
e 5 trata isso como uma revisão nova. O host a acha de novo na hora, no livro como está agora: no próprio
leitor, se o livro está aberto; senão num worker de render só dela, no sandbox (os caracteres de um livro
fixo), ou no livro normalizado (um fluido); e grava onde achou. O relatório da importação diz quantas
ficaram exatas, aproximadas e órfãs, e quantas esperam o livro abrir (o arquivo não está na pasta agora,
ou uma marca na imagem de uma página que ainda não foi traçada).

### D6. O relatório segue o livro

A ordem é a de leitura: num livro fixo, a página e a altura em que a marca começa; num fluido, a posição
do primeiro caractere no texto inteiro do livro. O sumário vem dos marcadores de um PDF (DjVu e quadrinhos
não têm, por ora) ou do sumário de um livro fluido, cada entrada no mesmo lugar em que o leitor a abre. Cada
anotação fica sob a entrada mais funda que começa antes dela; os títulos dos capítulos aparecem só onde há
anotações, com os de cima deles. As notas sobre o livro todo vêm primeiro; as não encontradas, por último,
numa seção à parte, com a citação, a nota e o lugar em que estavam. Uma linha por anotação diz o tipo (ou
*Caixa*, *Traço*, *Nota fixada*), a cor, a página ou quanto do livro, *aproximada* quando for, e a data.
Uma marca sem texto diz que é uma marca desenhada na página; recortes da imagem não entram (seção 5).

### D7. O Markdown

UTF-8, quebras de linha `\n`, front matter em YAML (título, pessoas, série, ano, idioma, identificadores,
formato, número de anotações, data e programa), a obra como `#`, os capítulos como `##` a `####`, a
citação como bloco `>`, a nota como parágrafos. O que um livro diz é texto: os caracteres que o Markdown
leria como estrutura são escapados, e também os do Obsidian (`#etiqueta`, `==realce==`, `%%comentário%%`,
`$math$`, `[[link]]`) e do GitHub (`~~`, `|`); `<` sempre, então nada vira HTML. A nota é lida como o
leitor a lê (D8) e escrita de novo a partir disso: os títulos dela descem para baixo dos do relatório; um
bloco de código que ela não fechou é fechado; um link `http`, `https`, `mailto` ou `tel` continua link, e
os outros viram o texto e o endereço, como o leitor os mostra.

### D8. A nota é lida como o leitor a lê

O leitor tem o seu próprio Markdown (Fase 4): parágrafos, títulos, listas, citações, código, ênfase e
links, e nada mais. `musaeum-report` o reproduz em Rust, expressão por expressão, com o `\s` e o `.` do
JavaScript; um arquivo de casos comum é conferido pelos dois testes, o de TypeScript e o de Rust. Assim a
nota é a mesma no app, no Markdown e no PDF.

### D9. O PDF é escrito aqui, com fontes livres embutidas

Sem dependência nova: o arquivo, o subconjunto das fontes e a composição são deste crate, sobre o `flate2`
e o `blake3`, que já estavam aprovados. As fontes são da família Noto, baixadas por
`tools/fetch-fonts.ps1` (fixadas num commit da distribuição do projeto Noto, com o SHA-256 de cada uma) e
embutidas no binário: Noto Sans (as notas, as linhas de cima, regular, itálico, negrito e negrito itálico),
Noto Serif (o que o livro diz, e os títulos), Noto Sans Mono (código), e Noto Sans Math e Noto Sans
Symbols 2 para o que as outras não desenham (setas, operadores, marcas de verificação). A licença é a SIL
Open Font License 1.1, que entra na lista só para o que é vendorizado (seção 8).

Cada fonte usada vai como `Type0` sobre um `CIDFontType2` com `Identity-H` e um `ToUnicode`, e o subconjunto
tem só os glifos usados, renumerados (com as partes dos glifos compostos), e as larguras em `/W`: o texto
selecionado e copiado é o que foi composto. Um caractere que nenhuma fonte desenha ganha um glifo vazio só
dele, que o `ToUnicode` mapeia para ele: aparece como uma caixa e é copiado certo. Letra mais acento
combinante viram o caractere composto (as letras latinas de Latin-1 e Latin Extended-A).

A página é A4, margens de 64 pt dos lados, o título da obra no alto (de todas menos a primeira) e
*página N de M* embaixo; cada anotação é um bloco com uma faixa à esquerda na cor da marca (a do realce, ou
a da tinta do sublinhado), a linha de cima em sans pequeno, a citação em serifado de 10,4 pt e a nota em
sans de 9,4 pt com negrito, itálico, código, listas, citações e títulos. Um título fica na página do que vem
depois; um bloco de até 14 linhas nunca é cortado (vai inteiro para a página seguinte); um maior é cortado
deixando ao menos 3 linhas de cada lado. A obra e os capítulos são os marcadores do PDF. Nada depende do
relógio: a data de geração é um dado do relatório, e o `/ID` é o hash do arquivo; o mesmo relatório dá os
mesmos bytes.

### D10. Onde os arquivos vão

JSON-LD, Markdown e PDF vão para onde a pessoa escolhe no diálogo do sistema, nunca dentro de uma pasta que
a biblioteca segue (a recusa compara os caminhos resolvidos, sem maiúsculas). Cada um é escrito num arquivo
temporário ao lado do destino, levado ao disco (`sync_all`) e renomeado; uma falha o apaga e deixa o que
havia. Um relatório em PDF é aberto pelo PDFium, num worker no sandbox, antes de tomar o nome: o número de
páginas tem de ser o composto e a primeira tem de ter texto.

### D11. Onde a pessoa acha as ações

- No leitor, em qualquer formato, a seção *Exportar e importar* no pé do painel *Anotações*, acima da *No
  arquivo PDF* (num PDF). Depois de uma importação, as anotações do livro são lidas de novo.
- Nos detalhes de uma obra, a seção *Anotações*: JSON-LD nos dois sentidos e os dois relatórios.
- Nos detalhes de vários livros escolhidos, *Anotações dos escolhidos*: um JSON-LD e um relatório de
  todos (importar é da obra ou da biblioteca).
- Na barra da biblioteca, o menu *Anotações*: exportar todas, importar.

## 4. Resultados

Build release da `fase-7`, com a máquina livre: sem entrada havia 3 minutos, sem jogo e sem vídeo à frente.

**Testes automáticos.** São 824 testes de Rust (eram 795 na Parte 1), todos passam, e nenhum é ignorado.
De TypeScript são 215 (eram 212), com 1 pulado, o opcional dos escaneados reais da Fase 5. `fmt`,
`clippy -D warnings`, `tsc` e as licenças estão limpos. Os novos:

- 17 em `musaeum-report`: a ordem e os capítulos, o Markdown das notas pelos casos comuns, o escape, as
  datas, as fontes, o subconjunto e a paginação.
- 1 com o PDFium, em `musaeum-render-pdf/tests/report.rs`. Ele confere os mesmos bytes duas vezes, o
  texto e os acentos extraídos, *página N de M*, os marcadores, as seis cores das faixas medidas nos
  pixels e uma citação longa cortada entre páginas.
- 3 de integração do intercâmbio na biblioteca. O primeiro leva tudo a uma biblioteca limpa e compara,
  item por item, o arquivo que ela escreve de volta. O segundo põe as anotações numa outra edição, pela
  obra. O terceiro confere que 13 arquivos hostis são recusados sem gravar nada.
- 1 do OCR no índice do texto (seção 7, item 2).
- Os de unidade do host: os nomes dos arquivos e a escrita atômica.
- No TypeScript, os casos comuns das notas e os textos das ações.

**Na janela real** (`tools/e2e-exchange.mjs`): 34 de 34 passos, em 56 s.

| Parte | O que se confere |
|---|---|
| Anotar | Num PDF de texto, uma nota com Markdown e acentos, um realce e um sublinhado. Num PDF com `/Rotate 90`, um realce azul. Num escaneado, uma caixa cuja citação o OCR leu. Num EPUB, três notas, cada uma com a citação do seu parágrafo |
| Do leitor | O JSON-LD do livro (3 anotações). O relatório em Markdown, com o front matter, a ordem de leitura e a nota como foi escrita. O relatório em PDF, de que o PDFium extrai o texto, cada acento, a nota e *página 1 de 1*. O mesmo relatório duas vezes dá os mesmos bytes. Gravar numa pasta da biblioteca é recusado |
| Índice do texto | Uma palavra da citação da caixa no escaneado (*estantes*) acha o escaneado |
| Biblioteca | O menu *Anotações* exporta as 8 anotações, de 4 documentos, com os 7 tipos de seletor. Nos detalhes do EPUB, o relatório tem os capítulos *Primeiro* e *Terceiro*, com cada anotação sob o seu, e sai também em PDF. Com os 4 livros escolhidos, sai um relatório com uma seção por obra e um JSON-LD de todos |
| Idempotência e arquivos hostis | Trazido de novo, o arquivo acrescenta 0 anotações (8 já estão lá). Cinco arquivos são recusados e nada muda: um `@context` remoto, aninhamento maior que 16, uma chave repetida, um arquivo que não é JSON e um total que não confere. Os livros ficam iguais byte a byte, e nenhum temporário fica ao lado do que foi gravado |
| Biblioteca limpa | As 8 voltam, todas exatas, e o JSON-LD que ela escreve é igual ao original, item por item. Trazidas de novo, nada muda. O realce verde é desenhado no lugar, conferido no pixel. O relatório em PDF, aberto no Musaeum, mostra a faixa amarela da primeira anotação |
| Segundas edições | Entram 6 anotações pela obra (o PDF pelo `/ID[0]`, o EPUB pelo identificador). Em cada livro, 1 fica exata, 1 aproximada e 1 órfã, e o leitor mostra os três estados. *girado* e *escaneado* aparecem como obras que a biblioteca não tem, cada uma com 1 anotação esperando |
| Livros reais, copiados | MacKay (*Information Theory…*) e Mankiw (*Introdução à Economia*): as 5 anotações do PDF de cada um saem em JSON-LD e num relatório que o PDFium lê. Trazidas de volta, não acrescentam nada. O livro no acervo fica igual |

**Regressões na janela**, com o mesmo build, todas passam:

- as anotações levadas ao PDF da Parte 1 (23 de 23);
- as anotações da Fase 4 (fixtures em PDF, EPUB e DjVu; fechar e abrir, renomear, mover, segunda edição);
- o OCR da Fase 5 (15 de 15);
- o Beta sintético da Fase 6 (24 de 24);
- a taxonomia (15 de 15).

**O acervo**, só lido, tem 191 arquivos, 84 pastas e 8.877.205.812 bytes, antes e depois. Cada arquivo
ficou com o mesmo tamanho, a mesma data de escrita e os mesmos atributos; a escrita mais recente é de
21/11, antes desta fase. Dele, só MacKay e Mankiw foram copiados, para uma pasta temporária.

**Limpeza:**

- Nenhum processo do Musaeum nem WebView2 dele ficou aberto.
- No `%TEMP%` não ficou nenhuma pasta das passadas (`musaeum-e2e-*`) nem dos testes (`musaeum-test-*`).
- Na Lixeira não ficou nada dos testes.

**O caminho.** A primeira cadeia na janela parou três vezes na passada nova. Duas falhas eram do roteiro.
Ele esperava as linhas da imagem de um escaneado, que só são traçadas com a ferramenta de linhas. Também
procurava um relatório pelo nome, quando ele e a cópia feita para conferir os bytes são um livro só. As
outras falhas eram os defeitos 2 e 3 da seção 7. Um teste antigo do worker
(`a_worker_that_crashes_on_a_book…`) falhou uma vez, rodando ao mesmo tempo que um build release. Com a
máquina carregada, o worker não respondeu em 1,5 s, e o livro ficou para ser lido depois, que é o
comportamento previsto para um worker que não sobe. Em todas as outras corridas ele passou.

## 5. Limitações conhecidas

- **Sem kerning nem ligaduras, e só escritas da esquerda para a direita.** O texto é composto glifo a
  glifo pelas larguras da fonte; árabe e hebraico sairiam na ordem errada; chinês, japonês e coreano não
  têm fonte embutida (caixas vazias, copiadas certas).
- **Marcas sem texto não levam recorte** da imagem para o relatório: a linha diz que é uma marca desenhada
  e em que página.
- **DjVu e quadrinhos não têm sumário** no relatório (a Fase 2 não lê o do DjVu).
- **`TextPositionSelector` em unidades de UTF-16**, marcado em `musaeum:unit`: só difere dos pontos de
  código do W3C fora do plano básico (emojis, ideogramas raros).
- **Outra edição de um PDF casa pelo `/ID[0]` ou por um identificador**: a biblioteca não lê título e autor
  de dentro de um PDF (D4).
- **Anotações cujo livro não está na pasta** entram, mas ficam esperando o livro abrir para serem achadas.
- **A exportação para a cópia do PDF e o XFDF da Parte 1** ainda podem ser gravados numa pasta da
  biblioteca (a cópia vira um livro dela); a recusa desta parte vale para os formatos dela (seção 8).
- **O `cache.db` passa à versão 7** (seção 7, item 2). O Beta (v0.2.0), se aberto depois na mesma
  biblioteca, acha um cache mais novo que ele e o refaz, como sempre fez: o cache só guarda o que pode ser
  calculado de novo (geometria, linhas, OCR, índice do texto), e nada da biblioteca se perde.

## 6. Como reproduzir

```powershell
powershell -ExecutionPolicy Bypass -File tools\fetch-fonts.ps1
cargo test -p musaeum-report
cargo test -p musaeum-library --test exchange
cargo test -p musaeum-render-pdf --test report
cd apps\desktop; npm test; npm run build:app; cd ..\..
node tools\e2e-exchange.mjs
```

## 7. Defeitos achados no caminho

1. **O Markdown das notas do leitor travava numa linha que começa como título e não é um** (achado ao
   reproduzi-lo em Rust): `# a` seguido de U+2028 (separador de linha) passa pelo teste de fim de parágrafo
   (`#{1,6}\s`) mas não pelo de título (`.` não casa U+2028), e o laço de parágrafos não avançava, para
   sempre, travando a aba. Agora uma linha assim é um parágrafo dela; o caso está no arquivo comum.
2. **O que o OCR lia num escaneado nunca chegava ao índice do texto** (Fase 6, Parte 2; achado na passada
   na janela, que agora procura no índice uma palavra da citação da caixa feita no escaneado). Eram dois
   defeitos:
   - `ocr_page_texts`, que dá ao índice as linhas lidas de uma página sem camada de texto, esperava cada
     linha como uma lista de objetos `{text}`. O leitor a grava como o esquema do cache diz
     (`0005_ocr_line.sql`): `{"words": [[texto, de, até, confiança], …], "confidence", "flipped"}`.
     Nenhuma linha era aceita, e um escaneado lido pelo OCR ficava *vazio* no índice. Nenhum teste
     cobria essa função; agora um cobre, com as linhas na forma em que o host as grava, e o teste do host
     confere essa forma.
   - Nada acordava o leitor do índice quando o OCR gravava linhas novas. O `fulltext_status` dizia que o
     escaneado estava pendente, mas o leitor só olhava de novo num evento das pastas ou no próximo início
     do app (o comando `fulltext_wake` existia, e ninguém o chamava). Agora gravar linhas lidas pelo OCR
     o acorda. Enquanto o livro está aberto, ele continua cedendo ao leitor.

   A migração 7 do `cache.db` zera a contagem de linhas guardada com cada arquivo já indexado. Assim, uma
   biblioteca que já tinha escaneados lidos relê as páginas deles, e só as deles, sem reler o resto do
   acervo, o que mudar o `FULLTEXT_VERSION` faria.
3. **Um número podia voltar do JSON-LD diferente no último bit** (achado na passada na janela: a posição de
   duas notas do EPUB, `0,0014092306675042153`, voltava da biblioteca limpa como `…151`). Sem a feature
   `float_roundtrip`, o `serde_json` lê números com ponto por um caminho rápido que às vezes erra o último
   bit; a escrita já era exata. A feature está ligada no workspace (sem crate novo), e o teste da
   biblioteca agora usa esse número e compara, item por item, o arquivo que a biblioteca limpa escreve de
   volta com o original.

## 8. Pontos para a auditoria

1. **Uma licença nova na lista, só para fontes vendorizadas: a SIL Open Font License 1.1** (D9). Ela deixa
   embutir as fontes em documentos e distribuí-las com qualquer programa; só as fontes ficam sob ela.
   `tools/check-licenses.mjs` a aceita apenas no que é vendorizado.
2. **As fontes são as da família Noto** (Sans, Serif, Sans Mono, Sans Math, Sans Symbols 2), baixadas e
   conferidas por `tools/fetch-fonts.ps1`, fora do repositório (cerca de 4,4 MB), embutidas no binário e
   nos PDFs só em subconjuntos.
3. **O vocabulário próprio é `urn:musaeum:annotation:`** (D1), um nome sem domínio; se o projeto tiver um
   domínio, a versão 2 do formato pode usá-lo.
4. **`TextPositionSelector` conta em UTF-16**, como a biblioteca, e o diz em `musaeum:unit` (seção 5).
5. **A identidade é o UUID** (D3): as apagadas não vão para o arquivo, e um arquivo antigo não traz de
   volta uma anotação apagada nem desfaz uma edição. A alternativa seria comparar `modified` e ficar com a
   mais nova.
6. **Para outra edição, um identificador ou o título e o primeiro autor; nunca o título sozinho** (D4).
7. **No livro escolhido entra o arquivo de uma obra só, mesmo de outra obra**, e o relatório avisa (D4,
   item 3), como o XFDF de outro PDF da Parte 1.
8. **Marcas sem texto não levam recorte da imagem** para o relatório (seção 5).
9. **A recusa de gravar nas pastas da biblioteca vale para os formatos desta parte.** A proposta é
   estendê-la à cópia do PDF e ao XFDF da Parte 1, que hoje a pessoa ainda pode gravar ali (a cópia vira um
   livro da biblioteca).
10. **Um relatório em PDF é conferido pelo PDFium antes de tomar o nome** (D10), o que leva cerca de um
    segundo a mais.
11. **Os rótulos dos relatórios estão em português**, como `/Subj` na Parte 1; passam pela tradução da
    interface na Fase 8.
12. **Um relatório é de um livro ou de obras escolhidas**; da biblioteca toda, só o JSON-LD (um relatório de
    centenas de livros num arquivo só pareceu menos útil que escolher).
13. **A correção do OCR no índice do texto (seção 7, item 2) toca a Fase 6.** Uma migração do cache relê
    só os escaneados que já tinham linhas lidas. A alternativa seria subir o `FULLTEXT_VERSION`, que
    relê o acervo inteiro numa pasta nova do índice.
14. **`float_roundtrip` ligado no `serde_json` do workspace** (seção 7, item 3), sem crate novo. Ler números
    com ponto fica cerca de duas vezes mais lento, uma parte ínfima de ler um arquivo, e cada número volta
    com todos os bits.

## 9. A auditoria (2026-11-23)

Os catorze pontos da seção 8 foram aceitos como estão implementados:

1. A SIL Open Font License 1.1 entra na lista, só para fontes vendorizadas.
2. As fontes são as da família Noto, embutidas em subconjuntos.
3. O vocabulário próprio é `urn:musaeum:annotation:`.
4. O `TextPositionSelector` conta em UTF-16 e o diz em `musaeum:unit`.
5. A identidade é o UUIDv7. As anotações apagadas não são exportadas nem voltam.
6. Outra edição casa por um identificador, pelo `/ID[0]` do PDF, ou pelo título com o primeiro autor;
   nunca só pelo título.
7. O arquivo de uma obra só, importado no livro escolhido, entra mesmo sendo de outra obra, com aviso.
8. As marcas sem texto não levam recorte da imagem para o relatório.
9. A recusa de gravar nas pastas da biblioteca vale também para a cópia do PDF e o XFDF da Parte 1. Essa
   extensão é uma mudança de código posterior à auditoria e entra na Fase 8, Parte 1, com testes.
10. Um relatório em PDF é conferido pelo PDFium antes de tomar o nome.
11. Os rótulos dos relatórios estão em português por ora; a tradução vem na Fase 8, Parte 1.
12. Um relatório é de um livro ou de obras escolhidas. Da biblioteca toda, só o JSON-LD.
13. A correção do OCR no índice do texto vem com a migração 7 do cache, sem subir o `FULLTEXT_VERSION`.
14. O `float_roundtrip` fica ligado no `serde_json` do workspace.

Os pontos 10 a 14 foram aprovados como escolhas técnicas já implementadas, que não arriscam o acervo nem
o contrato da 1.0.
