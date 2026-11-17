# ADR 0009: decisões da Fase 5, Parte 2, o texto de um livro escaneado

| | |
|---|---|
| **Data** | 2026-11-17 |
| **Situação** | Aceita na auditoria de 2026-11-17, com a emenda de D6 e as quatro correções da seção 10 |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo, WebView2 157 |
| **Escopo** | OCR por região num processo isolado (Tesseract 5.5.4), a cópia do texto de uma marca sobre a imagem, a citação da anotação preenchida pelo OCR e indexada no FTS5; o filtro de fotografias, retículas, gráficos e tabelas no detector de linhas; texto girado (90°, 270°, 45°, −30°) e linhas tortas; a validação na janela real e num acervo real, só lido |

## 1. Resumo

A Parte 1 achava as linhas de um livro escaneado e deixava marcá-las, mas uma marca não tinha texto:
não se copiava nem se buscava. Agora o texto sob qualquer marca (ao longo de linhas, uma caixa, um traço)
é lido: numa página escaneada, pelo Tesseract, num worker próprio no mesmo sandbox dos renderizadores,
linha a linha, cada linha endireitada ao longo da sua base antes de ser lida; numa página híbrida, pela
camada de texto que já está no arquivo com a ferramenta de texto e pelo nosso OCR com as outras (D6,
emendada na auditoria, seção 10.1). *Copiar* e Ctrl+C põem o texto na área de transferência como uma
seleção de um livro digital o põe (uma linha por linha, as palavras separadas por um espaço); a anotação
recebe esse texto como citação em segundo plano, uma vez, e a busca do Inspetor a acha por ele. O que
cada linha diz fica no `cache.db` e não é lido de novo.

O detector também mudou. Fotografias, retículas e as grades de tabelas deixam de virar linhas: num livro
de prova com uma fotografia em retícula, um gráfico e uma tabela, digitalizado de três jeitos, nenhuma
linha falsa ficou dentro deles (eram 4 no corpus sintético e 2 no livro real antes das correções). Blocos
de texto girados a 90°, 270°, 45° e −30° são achados e lidos no sentido certo (91 de 96 linhas no corpus
sintético; 16 de 18 no livro de prova limpo e no girado, 12 de 18 no curvado). As linhas do corpo do livro
de prova foram achadas todas (89 de 89 nos três gêmeos) e lidas exatamente em 88 a 89 delas. Na janela
real, a cópia de um livro escaneado saiu igual à do livro digital, palavra por palavra e linha por linha,
também a 90°, 45° e −30°. Num acervo real, só lido, o detector concorda com as linhas do corpo do texto
do OCR das páginas híbridas com F1 0,875 (0,644 na Parte 1), e o nosso OCR difere da camada de texto
delas em 2,2% dos caracteres.

O pedido dizia "conclua a integração do processo isolado de OCR (Tesseract)": não havia nenhum código de
OCR antes desta parte; o worker, a ponte em C++, a compilação do Tesseract e do Leptonica e todo o
caminho até a área de transferência são desta parte. E pedia "ajustar a projeção de Sauvola para eixos
rotacionados": a binarização de Sauvola é local e não depende da direção do texto, então ela continua
uma só, na página; o que gira é o resto, a máscara binária de cada bloco girado vai para o quadro do
bloco, onde as faixas, as colunas e as linhas são achadas por projeção ao longo do eixo do bloco (D9).

## 2. Entregas

| Entrega | Onde | Como foi conferida |
|---|---|---|
| Tesseract e Leptonica compilados do fonte | `tools/fetch-ocr.ps1` (fontes e modelos fixados por SHA-256); `crates/musaeum-tesseract` (`build.rs` lê o `SourceLists.cmake` do próprio Tesseract; ponte `shim/ocr.cpp`); `vendor/tesseract/README.md`; `vendor/third-party.json` | Testes do crate; `check-licenses.mjs` cobre o que o script baixa |
| O worker de OCR isolado | `crates/musaeum-ocr-worker` (`musaeum-ocr.exe`, `LockdownParser`), modelos enviados pelo host em bytes | `tests/worker.rs`: lê páginas reais desenhadas pelo PDFium, no sandbox; trava, morre, mensagens inválidas |
| O lado do host | `crates/musaeum-ocr`: protocolo, `OcrClient` (reinicia um worker que falha, tempo de 30 s, 768 MB), `strip` (a linha endireitada), `read_line` (e de cabeça para baixo, se lê melhor) | Testes de unidade |
| Imagem de qualquer região, em qualquer escala | `musaeum-core`: `RenderPool::request_region`, montada das tiles | Teste do pool do PDF |
| O que as linhas dizem, no cache | migração `cache/0005_ocr_line.sql`; `Library::ocr_lines`, `store_ocr_lines` | Teste da biblioteca: por versão do detector, do OCR e por idiomas |
| A citação preenchida uma vez | migração `library/0006_ocr_quote.sql` (o gatilho que guarda a citação aceita preenchê-la uma vez, de nada; a busca segue a citação); `Library::fill_quote` | Teste da biblioteca: preenchida, achada pela busca sem acento, uma segunda leitura não a muda, o banco recusa mudá-la |
| Texto de uma região | `src-tauri/src/ocr.rs`: comando `region_text`, a fila de citações em segundo plano, o evento `annotation-text` | 5 testes de unidade; janela (seção 5.5) |
| Cópia e citação na interface | `api.regionText`; `ReaderView` (lê ao desenhar, *Copiar*, Ctrl+C); `AnnotationStore.learned` | Testes do vitest; janela |
| Linhas em qualquer ângulo no compositor | `compositor/lines.ts`: `MLIN` v2, comprimento de arco, quads girados; *snap* longe de linhas não pega nada | 18 testes de `lines` (5 novos), o *snap* ≤ 1 ms |
| Detector: imagens, tabelas, texto girado | `crates/musaeum-vision`: `picture` (máscara de imagens), `components::lattice` (retícula), `orient` (blocos girados), `verify` (o veredito de cada linha), `detect`; `MLIN` v2 (`LINE_TURNED`, espaços como frações do arco) | 24 testes de unidade; corpus rotulado com 3 disposições novas (seção 5.1) |
| Livro de prova do OCR e os seus gêmeos | `gen-fixtures --only ocr` (`ocr.rs`); `gen_fixtures::scan`; `visual-check scan` e `page`; `corpus-check --ocr-book` | Seção 5.2 |
| Amostra real com OCR | `corpus-check --lines --ocr` | Seção 5.3 |
| Teste de janela | `tools/e2e-annotations.mjs --phase ocr`; `tools/lib/clipboard.mjs` | Seção 5.5 |

## 3. Como foi medido

- **Corpus rotulado** (`musaeum_vision::synth`), como na Parte 1, com três disposições novas: *crooked*
  (linhas tortas, cada uma inclinando e ondulando de um jeito, até 3°), *turned* (blocos girados 90°,
  270°, 45° e −30° ao lado de texto reto) e *graphics* (uma fotografia em retícula com legenda, um gráfico
  de barras com números no eixo, rótulos a 45° e título a 90°, e uma tabela com fios). O F1 agora vê o
  sentido: uma linha achada ao contrário não conta. "Falsa num gráfico" é uma linha achada que não é
  nenhuma das verdadeiras e tem metade ou mais da base dentro de um gráfico.
- **Livro de prova** (`corpus-check --ocr-book`): quatro páginas com fonte de verdade (Times e Helvetica
  padrão do PDF, com acentos), cada linha conhecida exatamente: prosa; duas colunas sobre uma
  fotografia em retícula com legenda e legenda a 90°; um gráfico; uma tabela com fios; blocos girados.
  O pool do leitor desenha o livro a 300 dpi e `gen_fixtures::scan` faz três gêmeos só de imagem (limpo;
  girado 1,5°; curvado na lombada e girado −0,6°). O detector roda na escala do leitor e o OCR lê cada
  linha achada, endireitada; o que ele lê é comparado com a linha verdadeira, caractere a caractere.
- **Amostra real, só lida** (`corpus-check --lines --ocr`): as mesmas 60 PDFs e 6 páginas de cada da
  Parte 1, sobre `C:\Marte\Archives\DigitalLibrary\Books`, listado antes e depois. Nas 15 páginas
  híbridas, cada linha do corpo achada que casa com uma linha da camada de texto (o OCR de outro
  programa) é lida pelo nosso OCR, como o leitor lê, e comparada com ela; a camada também erra, então a
  diferença é um teto do erro, não o erro.
- **Janela real** (`e2e-annotations --phase ocr`), seção 5.5.
- A máquina estava ocupada (um jogo em primeiro plano) durante parte das medições sem janela: os tempos
  de `detect` dessas corridas estão marcados; as janelas só abriram com a máquina livre.

## 4. Decisões

### D1. Tesseract, compilado do fonte, num worker isolado

Tesseract 5.5.4 só com o reconhecedor LSTM (`DISABLED_LEGACY_ENGINE`), sem janela de depuração e com
redes em precisão simples, e o Leptonica 1.87.0 sem nenhuma biblioteca de imagem: o Tesseract só recebe
pixels da memória. Os dois são ligados estaticamente a `musaeum-ocr.exe`, que roda no sandbox
`LockdownParser` com 768 MB no máximo e não abre arquivo nenhum: o host lê os modelos
(`tessdata_fast` 4.1.0, `por` e `eng`, Apache-2.0) e os envia em bytes; `TESSERACT_IMAGEDATA_AS_PIX`
mantém as imagens na memória, sem arquivo temporário. Um worker que trava (30 s) ou morre é trocado. O
OCR é a parte mais exposta do caminho: lê pixels de qualquer PDF; o sandbox é o mesmo dos renderizadores.

### D2. Linha a linha, endireitada pela base

O Tesseract não recebe a página: recebe cada linha que o detector achou, amostrada ao longo da sua
polilinha de base (`musaeum_ocr::strip`), com 1,8 altura-x acima e 0,75 abaixo e uma margem nas pontas,
e lê no modo de uma linha (PSM 7). Uma linha curva perto da lombada, a 45° ou a 90° sai reta. As palavras
voltam como trechos da linha, frações do comprimento da base: o mesmo jeito que uma marca ao longo de
linhas nomeia onde começa e termina, então as palavras de uma seleção saem por aritmética. Uma linha
lida com confiança média abaixo de 75 é lida de novo de cabeça para baixo, cortada outra vez com as
margens trocadas (de cabeça para baixo, maiúsculas e acentos ficam do lado que a primeira tira deixava
estreito, e saíam cortados), e fica a leitura que ganhar por 10 pontos. Palavras abaixo de 20 de confiança, barras de tabela e o que passa das pontas da linha
(o tique de um gráfico) ficam de fora.

### D3. Em que resolução

A página é desenhada pelo mesmo pool, na escala que deixa as letras pequenas das linhas pedidas com cerca
de 22 px de altura, entre 300 e 600 dpi, e só o retângulo que as linhas ocupam (`request_region`). No
livro de prova, a 300 dpi, o corpo saiu exato em 88 a 89 de 89 linhas.

### D4. O que uma linha diz fica guardado

`ocr_line` no `cache.db`, pelos bytes do arquivo, a página, o índice da linha no modelo, a versão do
detector, a do OCR e os idiomas. Uma seleção sobre linhas já lidas responde na hora, e a citação de uma
anotação sobre elas não lê de novo. Uma leitura de outra versão ou em outros idiomas conta como não lida.

### D5. A citação é preenchida uma vez, depois

Uma marca numa página escaneada nasce sem citação. A fila de citações (um thread em modo de segundo plano
do Windows) espera o modelo da página, lê as linhas sob a marca e grava o texto como citação, com um
seletor `text_quote`, uma vez: o gatilho da Fase 4 que impede mudar uma citação passou a aceitar
preenchê-la quando ela é nula, e só então; a busca (`annotation_words`, `annotation_trigrams`) segue a
citação preenchida. A página recebe a anotação pelo evento `annotation-text`. Ao abrir um documento,
marcas ainda sem citação entram na fila.

### D6. Copiar é igual a um livro digital

`region_text` responde o texto sob uma região: numa página com camada de texto (híbrida, ou DjVu com texto
oculto), os caracteres sob ela, como a seleção nativa os copia; senão, o OCR. As linhas vão em ordem de
leitura separadas por `\n` e as palavras por um espaço, como a cópia nativa. A interface pede o texto
assim que a região é desenhada, então Ctrl+C e *Copiar* já o têm; se ainda não, esperam por ele.

A proposta era que, numa página híbrida, o texto fosse sempre o da camada. A auditoria emendou isso
(seção 10.1): a ferramenta *Texto* usa a camada, como num livro digital; *Linhas*, *Caixa* e *Traço* usam
o nosso OCR, e a camada só quando o OCR não tem nada ali.

### D7. Fotografias, retículas, gráficos e tabelas

- A máscara de imagens (`picture`): células de cerca de 1,5 letra onde há textura (figuras com
  enchimento ≥ 0,2, blobs pequenos que não se comportam como letras, pontos de retícula), regiões
  retangulares preenchidas até a sua caixa. Uma linha com metade ou mais dentro dela não é texto.
- Retícula (`components::lattice`): pontos do tamanho de letras numa grade regular não são letras.
- Blocos girados com pontos de retícula em volta (0,15 ou mais por letra) não são texto: onde a retícula
  escurece, os pontos se fundem em blobs do tamanho de letras, em diagonais.
- Uma linha curta (até 4 letras de comprimento, em qualquer ângulo) com 3 ou mais pontos de retícula em
  volta é a borda cortada de uma fotografia, não uma palavra: exatamente os "riscos laterais e margens de
  imagens" do pedido.
- Tabelas: uma moldura de fios finos com uma grade dentro (fios finos nos dois sentidos, longe das
  bordas) é uma tabela, e uma linha dela atravessa as células, como uma camada de texto a lê. A borda de
  uma digitalização ou a sombra da lombada também cerca texto, mas o que a atravessa é grosso ou fica na
  borda (seção 6).
- O *snap* só pega uma linha a até 3 alturas de linha do ponteiro: um arrasto sobre uma fotografia não
  seleciona nada.

### D8. Linhas tortas

Cada linha segue a sua própria inclinação e curvatura (a polilinha da Parte 1), com as pontas da base
corrigidas por extrapolação (0,4 letra de tolerância) para um descendente não puxar o começo da linha, e
faixas do tamanho de um acento ignoradas. Duas faixas pequenas demais para serem uma linha sozinhas, a
até 0,3 letra uma da outra no mesmo troço, contam como uma: quando um troço só tem a última letra ou
duas de uma linha longa, o limiar que as outras linhas do troço impõem parte essas letras ao meio, e a
linha acabava antes delas.

### D9. Texto girado

Antes de tudo, as direções entre letras a 1,2–2,4 alturas umas das outras (um *docstrum*), dobradas em
meia volta: o texto da página é nível, e montes de direções a 20° ou mais dele são candidatos. Cada letra
vota com os vizinhos (2 votos ou mais, e mais que os de nível), as letras se ligam em cadeias e as cadeias
em blocos; pedaços de um bloco a menos de 12° se juntam quando as caixas dos seus membros se tocam, antes
e depois de o bloco absorver as letras soltas na sua área. Cada bloco vai para o seu próprio quadro (a
máscara da página girada para o bloco ficar nível) e é traçado como uma página; as linhas voltam para a
página. No quadro entram também as letras que o teste de retícula tomou por pontos: a 45°, as linhas de
um bloco formam uma grade de diagonais, e duas letras coladas num blob redondo ("Um", "rn") têm
vizinhos nos dois sentidos, como um ponto de retícula. Voltam ao bloco as que estão dentro da área dele,
têm o tamanho das suas letras e encostam numa delas; os pontos de uma fotografia ao lado ficam fora da
área ou longe das letras. Isso só muda o quadro que é traçado, não o filtro que decide se o bloco é
texto (D7). O sentido de leitura vem do perfil da tinta acima e abaixo da faixa das letras pequenas
(ascendentes são mais comuns que descendentes): com evidência de 0,10 ou mais, ela decide; com menos, uma
linha inclinada lê para a direita e uma quase de pé (até 15°) lê para cima.

### D10. O modelo `MLIN` v2 e o compositor

A versão 2 do modelo marca as linhas de blocos girados e mede os espaços entre palavras como frações do
comprimento da base, não da largura. O compositor mede tudo pelo comprimento de arco, com x e y na mesma
unidade: achar a linha sob o ponteiro, o *snap* às palavras e os quads (perpendiculares à linha,
emendados nas dobras) funcionam igual numa linha a 90° ou a 45°. Modelos v1 são recusados e a página é
analisada de novo. O *snap* ficou mais caro e foi reescrito sem alocação por segmento (seção 6).

### D11. Os portões

- Texto girado: revocação ≥ 0,90 das linhas giradas, lidas no sentido certo, e F1 ≥ 0,95 nessas páginas.
- Gráficos: nenhuma linha falsa dentro de fotografia, gráfico ou tabela, e F1 ≥ 0,95 no texto da página
  (prosa, legenda, linhas da tabela). Os rótulos do próprio gráfico (números de 1 a 3 caracteres, uma
  palavra de 3 a 5 letras a 45° sob cada barra) são curtos demais para o detector, que precisa de algumas
  letras em fila para ver uma linha: a revocação deles é medida e só é presa onde está (14%), para não
  piorar sem aviso. É uma mudança do portão que eu tinha escrito, feita às claras (seção 9).
- O tempo do *snap* no vitest é o menor de três medições de cada arrasto: os outros arquivos de teste
  rodam ao lado e um pedaço de tempo que eles tomam não é custo do *snap*.

## 5. Resultados

### 5.1 Corpus rotulado

| Disposição | Precisão | Revocação | F1 | Giradas | Falsas em gráficos |
|---|---|---|---|---|---|
| Reta | 0,997 | 0,988 | 0,993 | | 0 |
| Inclinada | 0,994 | 0,991 | 0,993 | | 0 |
| Duas colunas | 1,000 | 0,999 | 0,999 | | 0 |
| Duas colunas sobre foto | 0,991 | 0,986 | 0,988 | | 0 |
| Curva | 0,988 | 0,988 | 0,988 | | 0 |
| Torta | 0,994 | 0,989 | 0,991 | | 0 |
| Girada | 0,958 | 0,974 | 0,966 | 91/96 | 0 |
| Gráficos | 0,963 | 0,786 | 0,866 (texto: 0,971) | 6/31 | 0 |

Na disposição *graphics*, sem os rótulos do gráfico, F1 0,971; os rótulos, 7 de 47. Os portões (D11)
passam: revocação girada 0,948 (≥ 0,90) e F1 0,966 (≥ 0,95); nenhuma linha falsa em gráficos e F1 do
texto 0,971 (≥ 0,95); rótulos 14,9% (piso de 14%).

### 5.2 Livro de prova (fontes reais, três gêmeos escaneados)

| | Limpo | Girado 1,5° | Curvado |
|---|---|---|---|
| Corpo, achadas / exatas | 89/89 / 88 | 89/89 / 88 | 89/89 / 89 |
| Títulos | 4/4 / 4 | 4/4 / 4 | 4/4 / 4 |
| Linhas da tabela | 4/4 / 3 | 4/4 / 4 | 4/4 / 4 |
| Texto girado (90°, 270°, 45°, −30°) | 16/18 / 16 | 16/18 / 15 | 12/18 / 12 |
| Legendas | 1/2 | 1/2 | 1/2 |
| Rótulos do gráfico | 6/12 | 8/12 | 7/12 |
| Fólios | 0/4 | 1/4 | 1/4 |
| Linhas falsas em gráficos | 0 | 0 | 0 |
| `detect` mediana | 29,2 ms | 29,0 ms | 29,3 ms |
| OCR por linha, mediana | 35 ms | 34 ms | 34 ms |

O que sobra de erro: um "g" no começo de uma linha do corpo lido como "8" (limpo e girado) ou "&"
(girado), uma célula da tabela que perdeu o número do meio (limpo), e rótulos curtos lidos vazios. Uma
das duas legendas não é achada em nenhum dos gêmeos; nos gêmeos limpo e girado é a legenda a 90° ao lado
da fotografia. Antes das correções da seção 6, o texto girado era lido exatamente em 15, 14 e 12 linhas.

### 5.3 Amostra real, só lida

As mesmas 60 PDFs da Parte 1, 6 páginas de cada: 45 páginas escaneadas ou híbridas (30 e 15); as outras
310 têm texto e ficam de fora. O acervo foi listado antes e depois: 162 arquivos e 72 pastas, nada
acrescentado, mudado ou removido; só os tempos de último acesso, que o Windows desta máquina atualiza ao
ler.

| | Parte 1 | Parte 2 |
|---|---|---|
| Linhas do corpo contra o OCR das 15 híbridas: revocação | 0,886 | 0,903 |
| Precisão | 0,506 | 0,848 |
| F1 | 0,644 | 0,875 |
| Todas as linhas contra o OCR (legendas, cabeçalhos, rótulos) | | F1 0,693 (revocação 0,859) |
| `detect`, 39 páginas simples | mediana 18,0 ms, máx. 29,1 | mediana 24,7 ms, máx. 48,3 |
| `detect`, 6 páginas duplas (2258 × 1500) | 37,3 a 44,7 ms | 49,4 a 65,9 ms |

A precisão quase dobrou porque as linhas falsas em fotografias e gráficos sumiram (D7). O tempo foi
medido com a máquina livre, depois de três otimizações sem mudança de resultado (a mesma saída, byte a
byte, em 5 páginas): uma busca de vizinhos calculada e nunca usada, a classificação de texto nível feita
duas vezes, e uma divisão por pixel no cálculo dos momentos. Uma página dupla ficou cerca de 7 ms mais
rápida, mas ainda passa dos 50 ms do portão da Parte 1 (seção 9).

**O nosso OCR contra a camada de texto** das 15 páginas híbridas, cada linha do corpo achada lida como o
leitor lê (mediana de 303 dpi): 791 linhas, 563 iguais; 5,1% dos caracteres diferem. A camada também
é OCR, de outro programa, com outra tipografia (aspas, travessões, ligaduras): com a tipografia
igualada e só nas 768 linhas de comprimento parecido (entre 0,8 e 1,25 vez o da camada; nas outras 23,
uma comparação caractere a caractere não diz nada), 572 iguais e 2,2% de diferença. É um teto do nosso
erro, não o erro.

### 5.4 *Snap*

Sobre 140 linhas sintéticas, com arrastos de muitas linhas: mediana de cerca de 90 µs e percentil 99 de
cerca de 0,2 ms com os outros testes rodando ao lado (menor de três por arrasto). Antes da reescrita do
cálculo dos quads, 690 µs e 1,5 ms.

### 5.5 Na janela real

Build release congelado, a máquina livre (sem jogo aberto, ociosa há mais de 14 min), a área de
transferência do usuário guardada antes e devolvida depois.

| Passada | Resultado |
|---|---|
| `--phase ocr` (livro de prova nativo e o seu gêmeo só de imagem) | 15 de 15 |
| `--phase lines` (Parte 1, livro escaneado sem texto) | 11 de 11 |
| `--phase fixtures` (Fase 4: PDF, EPUB, reabrir, renomear, mover, segunda edição) | sem falhas |

Na passada de OCR:

- **Cópia.** Três linhas copiadas do livro nativo, com a ferramenta de texto, e do gêmeo escaneado, ao
  longo das linhas, saem iguais, caractere a caractere, com `\r\n` entre as linhas como a cópia nativa.
- **Citação.** A marca no escaneado cita o mesmo que a marca no nativo.
- **Caixa.** Uma caixa copia as linhas dentro dela.
- **Fotografia.** Uma caixa sobre a fotografia não tem texto, e copiá-la não põe nada na área de
  transferência. Um arrasto de linhas sobre ela não seleciona nada.
- **Texto girado.**
  - Uma caixa em volta de texto que sobe (90°) copia-o no sentido de leitura.
  - Quatro linhas a 45° e quatro a −30°, arrastadas, saem como se leem.
- **Busca e reabertura.** A busca acha a marca do escaneado por "numero", sem acento. Fechado e aberto de
  novo, a marca mantém a citação e o lugar.
- **Processos.** Nenhum worker de OCR fica rodando depois.

A primeira corrida com o build final desta parte falhou num ponto: a linha a −30° perdia "obra." (seção 6).
Depois da correção, a corrida passou inteira.

### 5.6 Testes automáticos

**703 testes de Rust** (eram 666 na Parte 1) e **187 de TypeScript** (eram 180), todos passando; um
arquivo do vitest fica de fora sem os modelos da amostra real (`lines-real.test.ts`), como na Parte 1.
`cargo fmt --check` e `cargo clippy --workspace --all-targets -D warnings` limpos; `check-licenses.mjs`
confere 24 componentes, e agora também o que `fetch-ocr.ps1` fixa (Tesseract, Leptonica, `tessdata`).

## 6. Defeitos achados no caminho

- **Blocos girados partidos.** O teste de "tocar" usava o centro de cada membro; numa página onde cada
  palavra é um blob comprido, o centro da primeira palavra do pedaço seguinte ficava longe. E a junção
  vinha só antes de o bloco absorver as letras soltas, quando as caixas ainda não se tocavam. Agora usa a
  caixa do membro e junta de novo depois. Revocação girada: 69 → 80 de 96.
- **Sentido com evidência fraca.** Um bloco a 30° com o perfil dizendo pouco (−0,12) era lido para cima,
  de cabeça para baixo. Em todos os blocos do corpus o sinal da evidência estava certo; o limiar desceu
  para 0,10 e, sem evidência, linha inclinada lê para a direita. 80 → 89 de 96.
- **Blocos girados na retícula** e **bordas cortadas da fotografia** (D7): 4 linhas falsas no sintético,
  2 no livro real; agora 0 e 0.
- **Molduras de tabela falsas.** Numa dupla digitalizada (amostra real), a borda escura da página
  cercava as três colunas de texto e contava como tabela: as linhas atravessavam as colunas e a revocação
  daquela página caiu de 32 para 1 linha. Uma tabela agora precisa de uma grade de fios finos dentro.
- **O *snap* ficou 9 vezes mais lento** com a geometria por arco (`Math.hypot` e arrays por segmento);
  reescrito em aritmética escalar.
- **A última palavra de uma linha girada.** Na janela, quatro linhas a −30° copiadas perdiam "obra." no
  fim da segunda; no livro de prova, o OCR lia "obr". Eram duas causas, achadas olhando a máscara do
  quadro do bloco. No último troço do quadro, o "a." vinha em duas faixas, cada uma abaixo do mínimo de
  tinta de uma linha, embora juntas passassem: o comentário do código dizia que um intervalo curto entre
  duas faixas fica dentro da mesma linha, mas a junção não existia (D8). E, a 45°, pares de letras
  coladas eram tomados por pontos de retícula e ficavam fora do quadro: "cadernos" começava no "d", e o
  "Uma" do fim da linha caía (D9). A primeira hipótese, colunas partidas dentro do quadro, estava
  errada: cada bloco era traçado como uma área de 4 linhas. No sintético, 90 → 91 de 96; no livro de
  prova, texto girado lido exatamente em 16 e 15 linhas, onde eram 15 e 14.
- **Português europeu** em duas mensagens novas da interface e no README do `vendor/tesseract`;
  corrigido para o português do Brasil do projeto.

## 7. Limitações conhecidas

- **Rótulos curtos de gráficos**: números de eixo e palavras de poucas letras a 45° são achados pouco
  (7 de 47 no sintético; 6 a 8 de 12 no livro de prova). Fólios (um número sozinho) também.
- **Legendas**: no livro de prova, uma das duas legendas falta em cada gêmeo; nos gêmeos limpo e girado,
  é a legenda a 90° colada à fotografia.
- **Tempo numa página dupla**: 49 a 66 ms numa imagem de 2258 × 1500 (duas páginas do livro), acima dos
  50 ms do portão da Parte 1; uma página simples fica abaixo (máximo de 48,3 ms).
- **Texto girado num scan curvado**: 12 de 18 linhas giradas no gêmeo curvado (16 nos outros dois). O
  quadro de um bloco é reto, e a curva da lombada o entorta; a causa exata não foi investigada nesta
  parte.
- **O "g" no começo de uma linha**: os modelos `tessdata_fast` leram "8" ou "&" num "g" inicial em 1 ou 2
  das 89 linhas do corpo; uma célula de tabela perdeu um número do meio ("Geral 12.480 3.150" lido
  "Geral 3.150").
- **Idiomas fixos**: português e inglês, os modelos distribuídos. Escolher outros fica para depois.
- **Um worker de OCR por vez**: uma caixa sobre uma página inteira lê todas as linhas dela (cerca de
  35 ms por linha), e uma cópia espera a fila de citações terminar a linha em que está.
- **Uma seleção numa página só**, como na Parte 1.
- **DjVu e quadrinhos** não têm modelo de linhas; num DjVu com texto oculto, a cópia usa esse texto.
- **O cache de linhas lidas** não é limpo; cresce com o que foi lido (algumas centenas de bytes por linha).

## 8. Como reproduzir

```text
powershell -ExecutionPolicy Bypass -File tools\fetch-ocr.ps1
cargo test -p musaeum-vision --release                    # corpus rotulado e portões
cargo test -p musaeum-library -p musaeum-ocr -p musaeum-ocr-worker -p musaeum-desktop
cargo build --release -p corpus-check -p musaeum-render-pdf -p musaeum-ocr-worker
target\release\corpus-check --ocr-book --work <pasta> --out ocr-book.json
target\release\corpus-check --lines --ocr --corpus C:\Marte\Archives\DigitalLibrary\Books ^
    --work <fora do acervo> --sample 60 --pages-per-file 6 --out real-ocr.json
cd apps\desktop && npm run build:app && cd ..\..
node tools\e2e-annotations.mjs --phase ocr

rem A auditoria (seção 10): páginas escolhidas de um livro do acervo, só lidas, e a janela sobre elas
target\release\corpus-check --inspect <PDF do acervo> --at 79+82 --corpus <acervo> ^
    --work <fora do acervo> --ocr --out tm9.json
node tools\e2e-annotations.mjs --phase real --corpus <acervo> --real tm9.json+vm.json
```

## 9. Pontos para a auditoria

Respondidos em 2026-11-17 (seção 10.1). Um só commit (1), o tempo da página dupla aceito por página
lógica (2), a página híbrida emendada (4), idiomas fixos e o cache sem limpeza até a Fase 8 (7). Os
pontos 3, 5, 6 e 8 ficaram como estão. A lista abaixo é a que foi à auditoria.

1. **O commit.** A Parte 1 nunca foi aprovada nem gravada: as duas partes estão juntas na árvore de
   trabalho da `fase-5`, e a Parte 2 mudou arquivos da Parte 1 (o `MLIN` v1 virou v2, o detector, o
   compositor). Proponho um commit só, "Fase 5, Partes 1 e 2", com os ADRs 0008 e 0009; separar as duas
   agora seria inventar um estado da Parte 1 que nunca foi testado sozinho.
2. **O tempo do detector numa página dupla** (seção 5.3). O portão da Parte 1 era de 50 ms por página.
   Uma página simples continua abaixo; uma página dupla (duas páginas do livro numa imagem só) passa
   disso com o que a Parte 2 acrescentou (retícula, momentos, blocos girados, máscara de imagens,
   tabelas). Aceitar por página do livro, ou pedir mais otimização antes de fechar a fase?
3. **O portão dos rótulos de gráfico** (D11): mudado às claras. O F1 ≥ 0,95 vale para o texto da
   página; os rótulos curtos do gráfico são medidos e presos onde estão (14%).
4. **Página híbrida copia a camada de texto** (D6), mesmo quando ela vem de um OCR antigo e pior que o
   nosso. Assim a cópia é a mesma que a seleção nativa daria. A alternativa seria preferir o nosso OCR, ou
   oferecer os dois.
5. **A redação do pedido.** O pedido dizia "conclua a integração do Tesseract", mas não havia OCR antes
   desta parte; tudo foi feito aqui. Dizia também "projeção de Sauvola para eixos rotacionados", mas a
   binarização é local e continua uma só; o que gira é a projeção, no quadro de cada bloco (seção 1).
6. **O tempo do *snap* no vitest** é o menor de três medições por arrasto (D11), porque outros arquivos
   de teste rodam ao lado.
7. **Idiomas fixos** (português e inglês) e o cache de linhas lidas sem limpeza (seção 7): ficam para
   uma fase de configurações, ou entram agora?
8. **Os pontos em aberto da Parte 1** (ADR 0008, seção 9) continuam abertos. As linhas falsas em
   fotografias foram resolvidas aqui (D7). Seguem pendentes: rótulos reais, página dupla como duas
   lógicas, seleção visual numa página só e o hash da página como juiz.

## 10. A auditoria e as correções que ela pediu (2026-11-17)

### 10.1 As decisões

- **O commit**: um só, "Fase 5, Partes 1 e 2", com os ADRs 0008 e 0009.
- **Página dupla**: aceito o tempo por página do livro (uma imagem de duas páginas, cerca de 33 ms
  por página lógica).
- **Página híbrida** (emenda D6): com a ferramenta *Texto*, a camada de texto do arquivo, como num
  livro digital; com *Linhas*, *Caixa* ou *Traço*, o nosso OCR, e a camada só quando o OCR não tem
  nada ali (nenhuma linha sob a marca, o modelo de OCR ausente, as linhas ainda não traçadas). Uma
  página que não é imagem (digital) dá os seus caracteres e nunca chama o OCR; uma página ainda não
  classificada se comporta como antes (`ocr::region_reading`, com teste das combinações). A citação
  de uma marca numa página híbrida passa a vir do OCR, em segundo plano, como numa escaneada.
- **Idiomas e cache**: português e inglês fixos; o seletor de idiomas e a limpeza do cache de linhas
  lidas ficam para a Fase 8.

### 10.2 As quatro falhas achadas à mão, e o que as causava

As quatro foram reproduzidas antes de corrigidas, com um modo novo do `corpus-check`, `--inspect`,
que abre páginas escolhidas de um livro do acervo (só lendo; o acervo é listado antes e depois),
acha as linhas, lê cada uma com o OCR e grava a imagem da página com as linhas desenhadas.

1. **Sumário com pontilhado** (TM9-1005-213-10, página 82: "ENEMY USE ____ 5-1 89" pulada). A linha
   era achada e descartada pela verificação por altura: os tracinhos do pontilhado são componentes
   baixos e, mais numerosos que as letras, puxavam a mediana das alturas para baixo do limite.
   Correção (`verify.rs`): marcas baixas sobre a base (pontos, traços, sublinhados, pontuação) ficam
   fora do julgamento das letras quando a linha tem pelo menos 3 letras de verdade; uma fila só de
   marcas continua não sendo linha.
2. **Linha curta em itálico** (Vida Maravilhosa, página 14: "Wiwaxia, 215" pulada). A linha era achada
   e descartada por alinhamento: com só duas faixas, a mediana das alturas pegava a maior, a dos
   algarismos, e as minúsculas em itálico não cobriam esse x-height. Correção (`detect::keep_lines`):
   uma linha descartada por altura ou alinhamento que está entre duas linhas aceitas da mesma área,
   cada uma a um passo de linha (0,5 a 1,6 vez o passo da área), é julgada de novo com o x-height,
   a ascendente e a descendente delas, e fica com eles se passar. Uma fila de pontos ou de sujeira não
   passa nem assim.
3. **Sublinhado a lápis e rabiscos na margem** (Vida Maravilhosa, página 17: "novamente a partir desse"
   lida "a partir © ia /", a linha descendo para "cia" da linha de baixo). Eram quatro causas juntas:
   - os rabiscos da margem abriam uma "calha" ao lado da coluna, que cortava a página em faixas
     horizontais; a coluna dos rabiscos, quase vazia, era devolvida, mas o corte ficava, e numa
     página inclinada 3° ele parte as linhas ao meio (`lines.rs`: uma faixa cujas colunas, depois
     disso, são as da faixa de cima volta a fazer parte dela);
   - traços de caneta ou lápis contavam como letras (`detect.rs`: um componente com mais de 2,5
     letras de comprimento, menos de um quarto de letra de espessura e cheio ao longo do eixo é um
     traço, não texto; fora de imagens, que já têm os seus);
   - sublinhados colados às letras (`Bitmap::erase_underlines`: tinta com até 0,22 de letra de
     altura numa corrida horizontal de 1,5 letra ou mais sai da máscara das linhas; onde cruza a
     haste de uma letra não é fina, e a haste fica);
   - no último troço da coluna, os rabiscos fundiam as faixas de duas linhas numa só, cujo centro
     ainda caía onde a linha de cima previa, mas cuja base era a da linha de baixo
     (`find_lines_with_tables`: a base da faixa também tem de cair onde a linha prevê; `trace_baseline`:
     do meio para as pontas, nenhum passo dobra a linha mais de 12° do rumo que ela trazia, mais meia
     letra de folga, o "max dy/dx" pedido; o rumo é a mediana dos três últimos passos aceitos, para
     que um expoente de fórmula não o vire).
   E a ponta de duas linhas ainda ia até um pedaço de rabisco na margem: numa coluna justificada
   (60% das linhas longas acabando a meia letra do mesmo lugar), a tinta além da borda, depois de um
   espaço maior que o entre palavras, é cortada da linha (`to_column_edges`); numa coluna de margem
   irregular, como um sumário, nada é cortado.
4. **Página impressa de lado** (TM9, página 79 do arquivo; 78 é a página em branco ao lado no par):
   todo o texto está a 90° e só "Digitized by Google" a 0°. O detector sempre tomou o texto de uma
   página como nivelado e o resto como blocos girados; com quase tudo a 90°, os blocos ficavam
   pedaços, e as letras monoespaçadas formavam "linhas" a 0° entre linhas. Correção (`detect`,
   `orient::turned_text`): se as direções entre letras vizinhas a menos de 15° de vertical forem
   1,4 vez as horizontais ou mais, a página pode estar de lado; ela é analisada também girada um
   quarto de volta (no sentido em que as letras ficam em pé, decidido por elas) e fica girada se
   assim as suas linhas tomam claramente mais letras (+0,15 da fração), ou tantas quanto e os blocos
   em pé leem num só sentido. As linhas voltam ao referencial da página e são marcadas giradas.
   No compositor, um arrasto só toma linhas que correm no mesmo sentido da primeira, a menos de 30°
   (`lines.ts`: `sameWay`): para o fim do arrasto e entre o começo e o fim, de modo que o texto a 90°
   não se mistura com uma marca a 0° que o modelo põe entre elas.
   E uma linha de assinaturas colada à de baixo ("DONALD J. DELANDRO") lia vazio: a faixa do OCR
   pegava o topo da linha seguinte. Quando nem a leitura normal nem a de cabeça para baixo dão algo
   confiável, uma terceira lê uma faixa justa às letras da linha (`strip::straighten_tight`).

O modelo de linhas passou para a versão 3 (`LINES_VERSION`, Rust e TypeScript): modelos v2 em cache
são recusados e as páginas, traçadas de novo.

### 10.3 Dois defeitos das próprias correções, achados antes de fechar

- **Rabisco perto das letras.** Uma primeira versão da regra dos traços poupava um traço com duas
  letras a menos de 0,6 de letra (para não tomar o "f" longo de um itálico por traço). Na página 17 a
  caixa de um rabisco diagonal fica perto das letras mesmo com a tinta longe, e o rabisco voltava à
  máscara: a passada na janela leu "a partir dess". A regra foi tirada; o "f" não era o problema de
  "Chief of Staff" (seção 10.5).
- **Rumo de um passo só.** Na primeira versão do limite de dobra, o rumo era a inclinação do último
  passo aceito: num livro de Cálculo, um expoente aceito virou o rumo em 13°, e o resto da linha foi
  "corrigido" ao longo dele, subindo 75 px até a equação de cima; a folga de 0,15 letra descartava
  ainda a linha de uma fórmula. A amostra real mostrou a queda (F1 do corpo 0,875 → 0,853); a mediana
  dos últimos passos e a folga de meia letra a desfizeram.

### 10.4 Os números depois das correções

**As quatro falhas, de novo, no acervo** (`--inspect` com o build final, nas páginas 7, 21, 79, 82, 85 e 86
do TM9 e 14, 17 e 249 do Vida Maravilhosa; o acervo listado antes e depois, igual):

| Caso | Antes | Depois |
|---|---|---|
| TM9, p. 82, sumário com pontilhado | linha pulada | "ENEMY USE 5-1 89" |
| VM, p. 14, linha curta em itálico | linha pulada | "Wiwaxia, 215" |
| VM, p. 17, sublinhado e rabiscos | "a partir © ia /", a linha descendo para a de baixo | "… a partir desse" e, na linha seguinte, "… semelhante à inteligência" |
| TM9, p. 79, página de lado | pedaços, misturados ao rodapé a 0° | "By Order of the Secretary of the Army:", "Distribution: To be distributed in accordance with DA Form", "DONALD J. DELANDRO" |

**Na janela real**, com o build release congelado e a máquina livre, a passada nova
`e2e-annotations --phase real` arrasta sobre esses mesmos lugares dos dois livros e confere o texto
copiado: 5 de 5. As pastas dos dois livros entram como raízes de uma biblioteca nova, numa pasta
temporária fora do acervo; o acervo é listado antes e depois, e não muda. As passadas de antes, `--phase ocr`,
`--phase lines` e `--phase fixtures`, passaram inteiras.

**Corpus rotulado** (o mesmo da seção 5.1):

| Disposição | F1 antes | F1 depois | Precisão antes → depois |
|---|---|---|---|
| Reta | 0,993 | 0,990 | 0,997 → 0,991 |
| Inclinada | 0,993 | 0,993 | 0,994 → 0,994 |
| Duas colunas | 0,999 | 0,999 | 1,000 → 0,999 |
| Duas colunas sobre foto | 0,988 | 0,986 | 0,991 → 0,986 |
| Curva | 0,988 | 0,991 | 0,988 → 0,991 |
| Torta | 0,991 | 0,989 | 0,994 → 0,989 |
| Girada | 0,966 | 0,962 | 0,958 → 0,950 (91 de 96 giradas, igual) |
| Gráficos | 0,866 (texto 0,971) | 0,866 (texto 0,971) | igual; rótulos 7 de 47, 0 linhas falsas |

Os portões (D11) passam. As pequenas quedas são todas de precisão (linhas a mais); a revocação ficou
igual em todas as disposições, e subiu na curva.

**Livro de prova**: igual à seção 5.2 em tudo o que é achado e lido (corpo 89 de 89 nos três gêmeos, 88,
88 e 89 lidas exatamente; texto girado 16, 16 e 12 de 18; nenhuma linha falsa em gráficos).
`detect` mediana 31,3, 32,0 e 31,7 ms (eram 29,2, 29,0 e 29,3); OCR por linha, 33 a 35 ms, igual.

Os tempos são de uma segunda corrida, com a máquina livre: na primeira, uma compilação do worktree da
Fase 6 rodou ao mesmo tempo e até a binarização, que as correções não tocam, saiu duas vezes mais lenta.

**Amostra real, só lida** (as mesmas 45 páginas da seção 5.3; o acervo igual antes e depois):

| | Antes | Depois |
|---|---|---|
| Linhas do corpo contra o OCR das 15 híbridas: F1 | 0,875 | 0,870 |
| Revocação | 0,903 | 0,916 |
| Precisão | 0,848 | 0,829 |
| Sem a página do mapa (abaixo): F1 | 0,887 | 0,890 |
| Nosso OCR contra a camada, tipografia igualada | 768 linhas, 2,2% | 777 linhas, 2,16% |
| `detect`, 39 páginas simples | mediana 24,7 ms, máx. 48,3 | mediana 27,7 ms, máx. 63,3 |
| `detect`, 6 páginas duplas | 49,4 a 65,9 ms | 52,5 a 67,2 ms |

O máximo de uma página simples passou dos 50 ms da Parte 1 numa só página: a 347 do *GENKI I*, japonês
escrito na vertical. As colunas de kana e kanji empilham as letras tanto na vertical quanto na
horizontal, então a página pode estar de lado; analisada girada, toma 0,84 das letras contra 0,74 como
está, menos que a margem de 0,15, e fica como está, com as colunas lidas como blocos a 90° e 270°. A
decisão é a certa; o custo é a segunda análise, 36 → 63 ms.

As correções acham 11 linhas certas a mais (a revocação sobe) e 34 a mais no total. Quinze dessas 34 estão
numa só página, um mapa do *Geografia Geral e do Brasil* (página 33) onde a camada de texto tem 9 linhas
longas, e o detector, com 56 blocos girados entre os nomes de lugares, acha 45 linhas longas, 15 a mais
que antes; sem ela, o F1 sobe de 0,887 para 0,890. Na página 503 do *Calculus*, olhada com `--inspect`, o texto corrido está todo achado e as linhas
que não casam com a camada estão nas figuras (a diagonal de um desenho, as duas legendas emendadas
numa).

**Os valores das decisões novas, medidos no acervo:** nas páginas de lado (TM9 7, 19, 21, 22 e 79; Vida
Maravilhosa 249 e 250), a fração de letras tomadas por linhas passa de 0,41 a 0,64 sem girar para 0,95 a
0,99 girada. Nenhuma página em pé foi girada: na página 2 do livro de prova, com blocos nos dois sentidos,
0,99 a 1,00 dos dois jeitos; nas páginas 85 e 86 do TM9, de tabelas, 0,63 contra 0,60 e 0,61 contra 0,71,
abaixo da margem de 0,15.

**Testes automáticos**: 714 de Rust (eram 703 na seção 5.6; os novos cobrem o pontilhado, o resgate, a
borda de coluna justificada, o sublinhado apagado, o rabisco na margem, a página de lado, a faixa justa
do OCR e a regra da página híbrida) e 189 de
TypeScript (eram 187: a página de lado no *snap*), todos passando, com `lines-real.test.ts` de fora como
antes. `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `tsc` e
`check-licenses.mjs` limpos.

### 10.5 O que ficou

- **"Chief of Staff"** na página 79: duas linhas em itálico tão juntas que as letras de uma ficam sobre
  as da outra na direção da inclinação, e isso parece texto a 107°; a linha sai em pedaços.
- **Custo**: uma página que pode estar de lado é analisada duas vezes (três se a primeira volta fica de
  cabeça para baixo); numa página que já se lê bem (0,85 das letras ou mais) e sem blocos de um sentido
  só, a segunda análise é pulada. Na amostra real, a página simples mais lenta é uma em pé que não foi
  pulada, a de japonês vertical (63 ms). A segunda análise binariza de novo a imagem girada; girar a
  máscara já binarizada pouparia cerca de 8 ms e, com a janela de Sauvola quadrada, deve dar o mesmo
  resultado (a conferir byte a byte), mas mexe no detector depois da validação, e ficou para a
  auditoria.
