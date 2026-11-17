# ADR 0008: decisões da Fase 5, Parte 1, as linhas de um livro escaneado

| | |
|---|---|
| **Data** | 2026-11-16 |
| **Situação** | Aceita na auditoria de 2026-11-17, junto com a Parte 2 (ADR 0009, seção 10); os pontos em aberto da seção 9 continuam abertos. O `MLIN` v1 daqui foi substituído pelo v2 e depois pelo v3 (ADR 0009) |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo, WebView2 157 |
| **Escopo** | O motor de visão `crates/musaeum-vision` (binarização de Sauvola, componentes conexos, inclinação, colunas por faixas, linhas como polilinhas, hash perceptual); o modelo de linhas no `cache.db`, feito em segundo plano para as páginas escaneadas e híbridas; o *snap* no worker do compositor e as três ferramentas de marcação sobre imagens (linhas visuais, caixa, traço livre), guardadas em `annotation`, `annotation_segment` e `anchor_selector` |

## 1. Resumo

Num livro escaneado não há texto para selecionar, só a imagem da página. O Musaeum passa a achar as
linhas de texto dessa imagem, sem nada aprendido: visão computacional clássica sobre uma imagem cinza de
1500 pixels de altura que os workers do PDF desenham em segundo plano. O resultado, um modelo de linhas
(polilinhas de base em frações da página, alturas, espaços entre palavras), fica no `cache.db` pelos
bytes do arquivo, e o compositor o usa para três ferramentas novas da barra do leitor: **Linhas** (um
arrasto seleciona da palavra onde começa até a palavra onde termina, as linhas do meio inteiras, seguindo
a curva das linhas perto da lombada; o ajuste às palavras pode ser desligado), **Caixa** (um retângulo
translúcido) e **Traço** (um marca-texto à mão livre). Cada marca vira uma anotação como as da Fase 4,
com cor, nota, Inspetor e busca, e se acha de novo pela página e pelo hash perceptual da imagem.

As metas do portão: F1 ≥ 0,95 nas páginas retas, F1 ≥ 0,85 nas curvas, análise ≤ 50 ms por página e
*snap* ≤ 1 ms. No corpus rotulado (150 páginas em cinco disposições) o F1 ficou entre 0,991 e 1,000 e a
análise em 23,9 ms de mediana, 33,5 ms no pior caso; nas 45 páginas escaneadas reais da amostra (só
lida), 18,7 ms de mediana e 44,7 ms no pior caso (uma página dupla, duas páginas do livro numa imagem);
o *snap*, 3 µs de mediana e 46,5 µs no percentil 99 sobre os modelos dessas páginas reais. Nas páginas
reais não há rótulos; nas 15 híbridas, as linhas do OCR são uma referência independente, e as linhas
do corpo do texto foram achadas com revocação de 0,886 (seção 5.3), com os limites da seção 7.

## 2. Entregas

| Entrega | Onde | Como foi conferida |
|---|---|---|
| Motor de visão | `crates/musaeum-vision`: `gray` (imagem integral), `sauvola`, `components`, `skew`, `lines` (faixas, colunas, linhas, linha de base), `model` (`MLIN`), `hash` (dHash de 64 bits), `detect` | 14 testes de unidade; o corpus rotulado (`synth`, `score`, `tests/corpus.rs`) |
| A imagem da página para o detector | `musaeum-core`: `BackgroundWork.lines`, `Work::Lines`, a imagem montada das tiles em cinza (`BackgroundResult::Lines`); `musaeum-shm::tile::lines_edge` | Teste do pool do PDF: três páginas de 1500 px montadas de nove tiles cada, 34 a 37 linhas achadas em cada |
| Trabalho pedido depois, e na frente | `RenderPool::add_background_first`, `PoolHandle` (`add_background`, `request_chars`), `Scheduler::add_background_first` e `promote_background` | Testes do agendador e do pool |
| O modelo no `cache.db` | migração `cache/0004_line_model.sql`; `Library::line_model`, `pages_without_line_model`, `store_line_models` | Teste da biblioteca: por versão, só páginas que o arquivo tem, os 64 bits do hash inteiros |
| O guardador em segundo plano | `src-tauri/src/background.rs`: classes, miniaturas e, para as páginas escaneadas e híbridas sem modelo, a análise e a gravação | Janela, fase `lines` (seção 5.5) |
| O modelo para o compositor | comando `line_model` (`src-tauri/src/lines.rs`): o hash e o `MLIN`, ou vazio enquanto a página não foi analisada, que a põe à frente do segundo plano | Janela |
| *Snap* e ferramentas | `compositor/lines.ts` (decodificação, `snapRange`, `pieceQuads`, caixa, traço, `dhashRgba`); `compositor/annotator.ts` (ferramentas `lines`, `rect`, `free`); `renderer.readRegion` | 19 testes do vitest (13 de `lines`, 6 das ferramentas), um deles o *snap* ≤ 1 ms; `lines-real.test.ts` sobre os modelos reais |
| Anotação de região | migração `library/0005_page_region.sql` (seletor `page_region`); `anchoring::region`; comando `create_region_annotation`; `anchor_fixed` reancora regiões | Testes da biblioteca (atualização da v4 com os gatilhos de volta; ida e volta de uma região); janela |
| Interface | barra do leitor fixo: Texto, Linhas, Caixa, Traço; *Ajustar às palavras* | Janela (seção 5.5) |
| Validação | `corpus-check --lines-synthetic` e `--lines` (só lê o acervo), `lines-real.test.ts` | Seção 5 |

## 3. Como foi medido

- **Corpus rotulado.** `musaeum_vision::synth` desenha páginas de 1060 × 1500 pixels com letras feitas de
  hastes, arcos, ascendentes e descendentes, espaços entre palavras, parágrafos com recuo e última linha
  curta, cabeçalho corrido e fólio, papel com grão e sujeira, tinta um pouco borrada, e guarda onde cada
  linha de base está de fato: as formas são desenhadas na página plana e levadas à imagem pela mesma
  deformação que dá as respostas. Cinco disposições: reta; inclinada (0,8° a 3°); duas colunas; duas
  colunas sobre uma fotografia de largura inteira, com legenda e uma coluna embaixo (*mixed*); e curva
  para a lombada como num scanner de mesa, as linhas descendo até 70 px e comprimidas perto da dobra,
  o papel mais escuro ali.
- **F1.** `musaeum_vision::score`: uma linha achada conta por uma verdadeira quando uma cobre ao menos
  70% da outra ao longo da página, nos dois sentidos, e as linhas de base distam em média no máximo meia
  altura-x onde se sobrepõem; cada linha conta uma vez, a mais próxima primeiro. Uma linha cortada em
  duas é uma perdida e duas a mais; duas linhas juntas, duas perdidas e uma a mais.
- **Amostra real, só lida.** `corpus-check --lines` sobre `C:\Marte\Archives\DigitalLibrary\Books`: o
  acervo (162 arquivos, 72 pastas) é listado com tamanhos, datas e atributos antes e depois, e a corrida
  falha se algo difere; biblioteca, cache, modelos e sobreposições ficam numa pasta de trabalho fora
  dele. Uma em cada 2 ou 3 das PDFs, 60 no total, seis páginas espalhadas de cada; classificadas pelo
  mesmo pool em segundo plano do leitor; as escaneadas e híbridas analisadas, com o tempo de `detect`.
  Nas híbridas, as linhas do OCR (a camada de texto) são a referência: uma reta pelas bases das caixas
  dos caracteres (mediana das inclinações, sem o quinto mais fundo). "Linhas do corpo" são as de ao menos
  oito letras de comprimento, dos dois lados.
- **Tempos.** `detect` em build release, a máquina livre (`machine-free2.ps1`), `corpus-check` abaixo da
  prioridade normal. O *snap* no Node do vitest, depois de 100 (sintético) ou 20 (real) chamadas de
  aquecimento, como o compositor já está quente quando o arrasto começa.

## 4. Decisões

### D1. Visão clássica, nada aprendido

Binarização de Sauvola (janela de 31 px, k = 0,3, sobre imagens integrais da soma e dos quadrados, em
blocos de 4 px), rotulação 8-conexa por união e busca, classificação dos componentes (glifo, sujeira,
figura, fio, borda) pela altura típica de letra, inclinação pelo ângulo que deixa mais nítido o perfil
das bases dos glifos, linhas por faixas verticais e ligação das faixas seguindo a tendência da linha. É
determinístico, explicável e rápido (≈ 20 ms por página), não depende de modelo nem de GPU, e uma página
que dá errado pode ser olhada passo a passo. Um modelo aprendido fica para quando houver rótulos reais.

### D2. Onde roda

O worker do PDF, no sandbox, desenha a página na escala do detector em tiles comuns, como trabalho de
segundo plano da menor prioridade; o pool monta as tiles numa imagem cinza (Rec. 601 em inteiros) e a
entrega ao guardador, um thread do processo principal em modo de segundo plano do Windows, que roda o
`detect` e grava o modelo. O worker não ganha código novo nem lê nada além do que já lia; a visão roda
fora do sandbox porque só lê pixels que o próprio Musaeum desenhou.

### D3. Quais páginas, e quando

As escaneadas e as híbridas (classes da Fase 2), sem modelo da versão atual do detector. As já
classificadas entram ao abrir o livro; as que a classificação descobre durante a sessão entram assim que
o lote de classes é gravado. Quando o leitor escolhe *Linhas* sobre uma página ainda sem modelo, o
comando `line_model` a põe na frente do segundo plano (ou adianta as tiles dela, se já esperavam), e o
compositor pergunta de novo a cada 500 ms, até 30 s. Isso vale para qualquer página, até de texto:
selecionar por linhas visuais sobre a imagem também serve ali.

### D4. Em que resolução

1500 px na aresta maior. Uma página em paisagem, quase sempre duas páginas escaneadas lado a lado, ganha
a altura de uma página só, até 3000 px de largura: desenhadas com 1500 px de largura, as letras dessas
duplas tinham 5 ou 6 px e as colunas não se separavam. O tempo por página do livro fica o mesmo.

### D5. Colunas por faixas da página

As colunas não são achadas na página inteira: a altura do texto é cortada em fatias de cerca de uma
linha, e uma calha é uma faixa vazia, larga o bastante (1,2 letra depois de cada glifo crescer meia letra
de cada lado), aberta em fatias seguidas por ao menos quatro linhas, com texto dos dois lados. A página
é dividida em faixas horizontais onde o mesmo conjunto de calhas está aberto, com as bordas levadas à
linha mais vazia ali perto; cada faixa tem as suas colunas, e a ordem de leitura é faixa por faixa,
coluna por coluna. Duas colunas sobre um gráfico de largura inteira e depois texto numa coluna são três
faixas. Só glifos do tamanho de letra contam (de 0,4 a 2,2 alturas de letra): o grão do papel e de uma
fotografia, em blobs de 2 ou 3 px, fechava todas as calhas de um scan. Quando as calhas falsas das
fotografias passam de 64 áreas (o limite do formato), as mais fracas, abertas ao lado de menos linhas,
saem primeiro. Uma linha que a borda da coluna cortou (um cabeçalho centrado sobre duas colunas) é
religada quando as bases se encontram e menos que uma calha separa os pedaços.

### D6. O modelo e o seu formato

`MLIN` versão 1: a rotação da página na análise, a inclinação, as colunas (caixas) e, por linha, a
coluna, a polilinha da base (pontos de cerca de duas letras em duas letras, simplificados por
Douglas–Peucker a 0,15 letra), a altura-x, a ascendente e a descendente (frações da altura da página
mostrada) e os espaços entre palavras (frações da linha). As posições são frações da *CropBox* antes da
rotação, o mesmo quadro dos quads de caracteres da Fase 4, então um modelo de hoje continua nomeando a
mesma tinta depois de uma revisão girar a página; a rotação vem da camada de caracteres da página, que o
guardador pede ao pool. O `cache.db` guarda o modelo, a versão do detector e o hash da página por
(BLAKE3, página); um modelo de outra versão conta como ausente.

### D7. O *snap* no worker do compositor

O compositor recebe o modelo de cada página visível quando a ferramenta *Linhas* está ligada e o
decodifica no quadro mostrado. Um arrasto acha a linha sob cada ponta (ou a mais próxima), ordena as
duas pela ordem de leitura e seleciona: a primeira linha do ponto de partida até o fim, as do meio
inteiras, a última do começo até o ponto final. Com *Ajustar às palavras* ligado, as pontas vão às bordas
das palavras em que estão (num espaço, à palavra seguinte no começo e à anterior no fim) e cada trecho é
desenhado por um quad por segmento da polilinha, seguindo a curva; desligado, as pontas ficam onde o
ponteiro está e cada trecho é uma caixa reta. Tudo isso é aritmética sobre algumas dezenas de linhas: 30
µs de mediana numa página densa de duas colunas, 3 µs nas páginas reais. Uma seleção visual fica numa
página: a da pressão.

### D8. Caixa e traço

A caixa é um retângulo da página, desenhado como uma marca (mínimo sobre papel claro) com uma borda fina
enquanto é desenhado; um clique sem arrasto não é caixa. O traço guarda os pontos do ponteiro a cada 1,5
pt e é desenhado como faixas de 9 pt, uma por passo, que se sobrepõem sem escurecer, porque a mistura é
o mínimo.

### D9. A anotação de uma região

Um seletor novo, `page_region`, com a página, a ferramenta, os quads (quadro guardado), a caixa (que o
`clip` do segmento repete), as linhas do detector cobertas (índice e frações), o caminho do traço, o hash
da página (do modelo no cache) e o hash da região (64 bits da diferença, calculado no compositor sobre
os pixels das tiles sob a região, lidos da textura do quadro). A migração 0005 refaz `anchor_selector`
com o mesmo conteúdo, ids incluídos, e os seus dois gatilhos, porque o SQLite não muda um `CHECK`. A
citação de uma região é o texto que a camada de OCR tiver sob ela (para a busca), ou nenhuma.

### D10. Achar uma região de novo

Não há texto para procurar. Na mesma revisão, a marca é a que foi feita. Numa revisão nova, a região
fica na mesma página e no mesmo lugar, e o hash da página decide: até 10 bits de diferença em 64 é a mesma
imagem desenhada de novo, e a marca é exata; mais que isso, aproximada; sem a página, órfã. Enquanto a
página da revisão nova não tem modelo, a anotação aparece onde estava e nada é gravado, para ser julgada
quando tiver.

## 5. Resultados

### 5.1 Testes automáticos

Na árvore final, 2026-11-16: `cargo fmt --check` limpo, `cargo clippy --workspace --all-targets -- -D
warnings` sem aviso, **666 testes de Rust** passando (eram 641 na Fase 4), **180 de TypeScript** (eram 160;
um a mais, `lines-real.test.ts`, só roda apontado para os modelos reais) e as licenças conferidas. Os
novos: 14 de unidade e 3 do corpus no `musaeum-vision`; 2 no agendador e 1 no pool do PDF (a imagem de três
páginas montada das tiles, e o trabalho acrescentado por outro thread); 1 de `lines_edge`; na biblioteca,
os modelos no `cache.db`, a atualização de uma biblioteca da Fase 4 com os gatilhos de volta, e 3 de
`anchoring::region`; 13 de `lines.ts` (decodificação, pontas nas palavras, a curva, o quadro guardado, o
traço, o hash igual ao do Rust, o *snap* ≤ 1 ms) e 6 das ferramentas do anotador. O hash perceptual do
compositor e o do Rust são o mesmo algoritmo: uma imagem fixada dá `268e1db4d2979746` nos dois.

### 5.2 Corpus rotulado

`corpus-check --lines-synthetic --pages 30`, build release, máquina livre (`bench-results/vision/synthetic.json`):

| Disposição | Páginas | Linhas verdadeiras | Precisão | Revocação | F1 | `detect` p50 | p95 | máx. |
|---|---|---|---|---|---|---|---|---|
| Reta | 30 | 1292 | 0,998 | 0,996 | **0,997** | 23,9 ms | 30,0 ms | 33,5 ms |
| Inclinada (0,8° a 3°) | 30 | 1345 | 0,999 | 0,996 | **0,997** | 22,7 ms | 28,2 ms | 32,8 ms |
| Duas colunas | 30 | 2599 | 1,000 | 1,000 | **1,000** | 21,0 ms | 25,7 ms | 25,9 ms |
| Duas colunas, fotografia, uma coluna | 30 | 1457 | 0,986 | 0,997 | **0,991** | 20,8 ms | 24,0 ms | 24,1 ms |
| Curva para a lombada (até 70 px) | 30 | 1305 | 0,998 | 0,995 | **0,996** | 23,5 ms | 26,7 ms | 28,3 ms |

As metas pedem F1 ≥ 0,95 nas retas e ≥ 0,85 nas curvas; o teste `tests/corpus.rs` as exige em oito
páginas de cada disposição a cada `cargo test`.

### 5.3 Amostra real, só lida

`corpus-check --lines`, 60 PDFs, até seis páginas de cada (`bench-results/vision/real-sample.json`): 355
páginas classificadas, 310 de texto (deixadas de lado), **45 analisadas: 30 escaneadas, 15 híbridas**,
de nove livros (química, entrevistas de programação, C++, aprendizado de máquina, japonês, lógica,
cálculo, geografia, processos de fabricação). O acervo listado antes e depois era o mesmo: 162 arquivos, 72 pastas; só a
data de último acesso, que o Windows atualiza ao ler, mudou em 132.

| | Valor |
|---|---|
| `detect` por imagem, p50 / p95 / máx. | 18,7 ms / 44,1 ms / 44,7 ms (as duplas de 2258 × 1500 px são as acima de 30 ms) |
| Linhas por página, p50 / p95 | 42 / 265 (os livros escaneados de uma coluna dão de 26 a 55, como o olho conta) |
| Contra as linhas do OCR, todas (15 páginas) | revocação 0,838, precisão 0,313, F1 0,456 |
| Contra as linhas do OCR, só as do corpo do texto | **revocação 0,886**, precisão 0,506, F1 0,644 |

A revocação diz que quase toda linha de texto que o OCR leu o detector também achou. A precisão baixa vem
de duas coisas que se veem nas sobreposições: fotografias e gráficos, onde rótulos, eixos e a textura de
meio-tom viram "linhas" (o OCR não lê nada numa foto), e rótulos curtos de figuras que o OCR quebra de
outro jeito. Nos livros de uma coluna o acordo é quase completo: *Calculus* (Strang), linhas do corpo
39 de 39, 37 de 38, 44 de 47. As referências do OCR também erram: em algumas linhas da *Geografia* as
caixas dos caracteres pulam.

### 5.4 *Snap*

| Onde | Chamadas | p50 | p99 | máx. |
|---|---|---|---|---|
| Página sintética densa, duas colunas, 140 linhas curvas (vitest) | 1000 | 30 µs | 322 µs | 821 µs |
| 45 modelos reais da amostra (`bench-results/vision/snap-real.json`) | 2250 | 3 µs | 46,5 µs | 860 µs |

O máximo é uma pausa do coletor de lixo do Node; o p99 está bem abaixo de 1 ms nos dois.

### 5.5 Na janela real

`tools/e2e-annotations.mjs --phase lines`, build release com a Fase 5, a máquina livre, 12 s, nenhuma falha
e nenhum erro de console (`bench-results/e2e-annotations/lines-3.json`). Um livro escaneado sem camada de
texto (`scanPdf` em `tools/lib/test-books.mjs`, duas páginas de letras desenhadas):

- ao escolher *Linhas*, o modelo da página aberta chegou do segundo plano, com **29 linhas, as 29 que a
  página tem**;
- um arrasto do meio da segunda palavra da linha 4 ao meio da terceira palavra da linha 6 selecionou a
  linha 4 da borda da segunda palavra até o fim, a 5 inteira e a 6 até o fim da terceira palavra;
  virou uma anotação de três quads, sem citação, e o pixel entre duas palavras da linha do meio ficou
  com a cor do realce, (168, 229, 161);
- com o ajuste às palavras desligado, as pontas ficaram onde o ponteiro estava (a menos de 1%);
- uma caixa sobre três linhas virou uma região de um quad, e o papel dentro dela ficou com o mínimo do
  azul e do papel do scan, (153, 201, 230) sobre (230, 230, 230);
- um traço na margem guardou 13 pontos;
- fechado e aberto de novo, as três marcas foram achadas **exatas**, desenhadas onde estavam.

As primeiras duas passadas falharam no teste, não no app: os pontos da tela eram calculados uma vez e
usados depois que a vista tinha andado, a sonda da caixa caiu na borda de uma letra, e a cor esperada
supunha papel branco. Agora cada ponto é calculado antes de usar e a cor esperada é o mínimo com o papel
medido ali antes da caixa. A passada das fixtures da Fase 4 com o mesmo build passou inteira (60 s), sem
regressão.

## 6. Defeitos achados no caminho

- **Calhas da página inteira.** A primeira versão achava as colunas por uma calha aberta em 80% da altura
  do texto; em páginas com duas colunas sobre um gráfico de largura inteira ela nunca abria, e as linhas
  atravessavam as duas colunas. Daí as faixas (D5) e a disposição *mixed* do corpus, que reproduz o caso.
- **Calha mínima pela largura.** O piso de 1,5% da largura dava 34 px numa dupla de 2258 px, mais que as
  calhas reais; passou a ser 1% do lado menor.
- **Grão do papel.** Blobs de 2 ou 3 px contavam como glifos e fechavam as calhas dos scans (D5).
- **Recurso de reserva demais.** Com mais de 64 áreas, a página inteira virava uma coluna; agora saem as
  calhas mais fracas.
- **Duplas pequenas demais** (D4).
- **Cabeçalho cortado pela coluna** (D5), achado no corpus de duas colunas.
- **`Buffer` do Node** no teste do *snap* real: um `Buffer` é uma vista de um *pool*, e o seu `.buffer` é
  o *pool* inteiro; o teste copia antes de decodificar.

## 7. Limitações conhecidas

- **Fotografias e gráficos** produzem linhas falsas (seção 5.3). Selecionar só acontece onde o leitor
  arrasta, então uma linha falsa numa foto só aparece se alguém arrastar sobre a foto; mas ela ocupa o
  modelo. Separar regiões de imagem antes das linhas fica para a Parte 2. *Tratado na Parte 2*
  ([ADR 0009](0009-fase-5-parte-2.md), D7): nenhuma linha falsa em gráficos no corpus nem no livro de
  prova.
- **Página dupla** é analisada como uma imagem: as colunas das duas páginas são faixas da mesma, e a ordem
  de leitura passa da página esquerda à direita faixa por faixa, não página por página.
- **Fórmulas e tabelas**: índices, frações e células viram linhas curtas próprias.
- **Rótulos reais de F1** não existem para as páginas reais; o OCR é uma referência imperfeita.
- **DjVu e quadrinhos** não entram nesta parte: só o worker do PDF desenha a imagem para o detector.
- **Uma seleção visual fica numa página.**

## 8. Como reproduzir

```text
cargo test -p musaeum-vision                              # unidade e corpus rotulado (F1)
cargo build --release -p corpus-check
target\release\corpus-check --lines-synthetic --pages 30 --out bench-results\vision\synthetic.json
target\release\corpus-check --lines --corpus C:\Marte\Archives\DigitalLibrary\Books --work <fora do acervo> ^
    --sample 60 --pages-per-file 6 --out bench-results\vision\real-sample.json [--dump <pasta>]
set MUSAEUM_LINE_MODELS=<pasta de trabalho>\line-models
npx vitest run src/compositor/lines-real.test.ts          # em apps/desktop
node tools\e2e-annotations.mjs --phase lines --build target\release
```

## 9. Pontos para a auditoria

1. **Linhas falsas em fotografias** (seção 7): aceitar para esta parte, ou separar regiões de imagem
   antes de fechar a fase?
2. **Revocação contra o OCR de 0,886 nas linhas do corpo**: é a melhor medida real que há sem rótulos;
   rotular à mão algumas páginas reais para um F1 de verdade?
3. **Página dupla** analisada como uma imagem (D4): dividir em duas páginas lógicas?
4. **Seleção visual numa página só** (D7).
5. **Hash da página como juiz da re-ancoragem** (D10), com 10 bits de tolerância.
