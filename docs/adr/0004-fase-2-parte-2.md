# ADR 0004: decisões da Fase 2, Parte 2

| | |
|---|---|
| **Data** | 2026-11-03 |
| **Situação** | Decisões aceitas. Em 2026-11-03, o projeto auditou e aprovou a Parte 2 e o portão da Fase 2, com as decisões da seção 8, e autorizou a Fase 3, Parte 1 |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo |
| **Escopo** | Classificação do conteúdo das páginas, miniaturas, quadrinhos (CBZ e CBR), DjVu, testes na janela real e comparação visual: o fechamento do portão da Fase 2 |

## 1. Resumo

O layout fixo agora lê três formatos, cada um num processo isolado que fala o mesmo protocolo: PDF pelo
PDFium, quadrinhos pela libarchive com decodificadores de imagem em Rust, DjVu pela DjVuLibre. Tiles,
geometria, páginas duplas, modos de leitura e a recuperação de workers são os mesmos para os três. Em segundo
plano, na menor prioridade do agendador, cada página é classificada como texto, escaneada ou híbrida, e ganha
uma miniatura em disco, pelo hash do arquivo.

Os resultados que mais pesam:

1. **Acervo inteiro classificado sem falha.** As 83.001 páginas dos 144 PDFs reais foram classificadas pelo
   worker no sandbox, sem uma falha: 86,4% texto, 5,2% escaneadas, 8,4% híbridas. O acervo continuou
   intacto (D2, seção 5.1).
2. **Quadrinhos e DjVu no sandbox mais estrito.** Os dois workers novos ligam tudo estaticamente e rodam sem
   win32k e só com DLLs da Microsoft, o que o worker de PDF não consegue (D8).
3. **Worker morto, leitura segue, nos três formatos.** Em release, com 8 mortes por formato no meio de render
   pesado, os três workers foram substituídos em P50 de 22 a 25 ms e máximo de 35 ms, e o tile pedido em seguida
   chegou em no máximo 51 ms. A meta é 1 s.
4. **Tiles iguais à página inteira.** A comparação visual achou um defeito da Parte 1: uma letra que atravessa
   a borda esquerda ou superior de um tile saía deslocada, até 131 níveis de diferença. Corrigido com uma margem
   em volta de cada tile (D16), os tiles reais do pool ficaram idênticos, pixel a pixel, à página desenhada
   inteira em todas as 15 páginas PDF das fixtures e em 9 de 12 páginas de livros reais; nas outras 3, a maior
   diferença é de 9 níveis em 0,25% dos pixels de uma ilustração.
5. **Na janela real, a posição não se mexe e a página sai igual.** Nos três formatos, trocar de modo pela barra e
   pelos atalhos, com e sem animação, deixou o Locator igual em todos os dígitos, e o `library.db` o devolveu
   exato ao reabrir. Das 27 páginas capturadas da janela, 22 são idênticas à página desenhada inteira e as outras
   5 diferem em no máximo 3 níveis. O zoom continua nítido no quadro seguinte ao fim da animação (seção 5.5).
6. **Cores do PDFium.** RGB sai exato, e as primárias CMYK ficam a ΔE00 de 0 a 0,28 do que o perfil SWOP dá. A
   janela mostra exatamente os números que o PDFium desenha. A sobreimpressão não é simulada (D13).
7. **Duas outras falhas encontradas no caminho.** O pool só conferia que o worker largou o token de
   inicialização na política `Lockdown`, então um worker na política mais estrita escaparia da verificação (D8).
   E o DjVuLibre só compõe uma página colorida se a camada preta tiver cor, o que as fixtures revelaram (D7).

## 2. Entregas

| Entrega | Como foi testado | Resultado |
|---|---|---|
| Commit da Parte 1 | `2ea3790` na `fase-2`, com o hook de licenças | Feito |
| Benchmark de zoom refeito | O cenário da Parte 1, nos mesmos dois livros, no build final, com o computador parado | Nítido em 13,3 a 13,4 ms nos 12 zooms, 0 quadros em branco; ADR 0003 atualizado (seção 5.5) |
| Classe de cada página e resumo | Fixture com as 8 situações (texto, escaneada, híbrida, imagem dentro de form XObject, figura de meia página, página vazia, página girada), o acervo inteiro, ida e volta no `cache.db` | Todas as classes certas; 83.001 páginas reais classificadas |
| Miniaturas em segundo plano | Agendador com sequências aleatórias, pool real nos três formatos, 8.028 miniaturas de 12 livros reais | Nenhum slot perdido; tiles visíveis primeiro; arquivos no disco pelo hash |
| Worker `render-raster` | ZIP deflate e sem compressão, RAR4, RAR5, ZIP com nome de CBR, páginas danificadas, arquivos que não são quadrinhos | Mesmas páginas e cores nos cinco arquivos; falhas isoladas |
| Worker `render-djvu` | Documento de 5 páginas com retrato, paisagem, quadrada, girada pelo arquivo e outra resolução, com camada de texto | Tamanhos, rotação, cores e texto certos |
| Recuperação em menos de 1 s | 8 mortes por formato, em release | Seção 5.2 |
| E2E na janela real | `tools/e2e-reader.mjs`, em PDF, quadrinho e DjVu: modos pela barra e pelos atalhos, com e sem animação, fechar e reabrir | 39 de 39 passos; Locator e posição salva exatos (seção 5.5) |
| Comparação visual e cores | `tools/visual-check`: tiles do pool contra a página inteira, janela contra a página inteira, retalhos de cor | Tiles: seção 5.3; cores: seção 5.4; janela: seção 5.5 |

**Testes.** 498 em Rust no workspace (eram 429; o último veio com a regra de maioria da auditoria) e 125 em TypeScript, em 19 arquivos, os mesmos da Parte 1: os
atalhos e a inspeção do leitor são exercitados pelo E2E. Todos passam, com `clippy -D warnings`, `fmt` e
`tsc --noEmit` limpos e as licenças conferidas (21 componentes vendorizados). As rodadas não deixaram nada no
`%TEMP%` nem na Lixeira. Um teste da Fase 1 falhou uma vez, de forma intermitente (seção 6).

## 3. Como foi medido

Como nas partes anteriores, nada aqui é simulação: workers reais, no sandbox real, sobre memória compartilhada
real.

- **Acervo.** `tools/corpus-check` ganhou uma fase: cada página de cada PDF é classificada como trabalho de
  segundo plano do pool, com 4 workers, e as classes vão para o `cache.db` do diretório de trabalho; os 12 livros
  da amostra ganham também as miniaturas. O acervo só é lido, e a lista de antes e depois de tudo comprova isso.
- **Quadrinhos e DjVu.** Não há nenhum no acervo, então são medidos com as fixtures do `gen-fixtures`. Os ZIP e
  RAR são escritos byte a byte e conferidos pelo 7-Zip; o DjVu é escrito pelos codificadores do próprio
  DjVuLibre (D7).
- **Recuperação.** O mesmo teste da Parte 1, agora no arnês comum (`musaeum-worker`, recurso `harness`): o único
  worker é morto de fora com `TerminateProcess` no meio de um render pesado, 8 vezes, e o leitor pede outra
  página logo em seguida. Os números da seção 5.2 são do build de release.
- **Concorrência.** O usuário jogava enquanto as medições das seções 5.1 a 5.4 rodavam, com elas abaixo da
  prioridade normal. Tempos são, portanto, piores que numa máquina parada; contagens, classes e pixels não mudam.
- **Janela real.** O benchmark de zoom e o E2E (seção 5.5) só começaram com o computador livre, conferido antes
  e depois de cada execução: sem entrada recente, sem jogo nem tela cheia na frente, GPU e CPU quietas, nenhum
  vídeo tocando. Uma execução em que alguém mexesse no computador seria descartada; nenhuma precisou ser.

## 4. Decisões

### D1. Um protocolo e um runtime para todos os workers

A versão 3 do protocolo acrescenta dois trabalhos, classificar uma página (`Analyze`) e ler a camada de texto
de uma página para um slot (`ExtractText`), e marca no pedido de render quando ele é de segundo plano, para o
worker não ocupar seus caches com o que ninguém está lendo. Tudo que não depende do formato, do handshake ao
laço de controle, mudou do worker de PDF para o crate `musaeum-worker`. Cada worker agora só abre o documento e
entrega um `Engine`; os três respondem igual porque o código que responde é o mesmo.

### D2. Texto, escaneada, híbrida: o worker mede, o host decide

O worker mede duas coisas por página: quantos caracteres a camada de texto tem, sem contar espaços, e que
fração da página as imagens cobrem, somando as que estão dentro de form XObjects numa grade de 64 × 64. O host
decide a classe com regras fixas em `musaeum-shm`:

- imagens cobrem menos de 85% da página: **texto** (inclusive página vazia ou só de vetores, porque nada nela
  foi escaneado);
- cobrem 85% ou mais, e há 16 caracteres ou mais de camada de texto: **híbrida**, o caso do OCR no arquivo,
  como o plano a define;
- cobrem 85% ou mais, e há menos: **escaneada**. O limite de 16 existe para um número de página carimbado num
  escaneamento não fazê-lo híbrido.

As medidas ficam no `cache.db` ao lado da classe, com a versão das regras: uma mudança de limite pode ser
julgada contra o que foi medido, e as páginas classificadas por regras antigas voltam a ser classificadas. Um
quadrinho é sempre escaneado; uma página DjVu é escaneada ou híbrida conforme a camada de texto.

O resumo do documento foi construído estrito: a classe, se todas as páginas a têm, e `mixed` se não. No acervo,
89 dos 144 documentos deram `mixed`, e a composição explica por quê: 59 têm 99% ou mais das páginas de texto, com
uma a três páginas que são só imagem (capas, pranchas); 11 são escaneamentos com OCR em que capas e páginas em
branco ficaram sem texto.

**Decisão da auditoria.** O resumo passou a ser a classe de pelo menos 95% das páginas, e `mixed` só abaixo disso.
A coluna `is_strictly_uniform` diz se todas as páginas têm a classe, e as contagens por classe (`text_pages`,
`scanned_pages`, `hybrid_pages`) continuam gravadas. Recalculado com as contagens da mesma rodada, sem medir de
novo, o acervo fica com 114 documentos de texto, 10 escaneados, 14 híbridos e 6 mistos, e 55 são estritamente
uniformes. Os 6 mistos de fato misturam: o que mais se aproxima tem 93% das páginas de texto (seção 8).

### D3. Segundo plano na menor prioridade

O agendador atende, nesta ordem: a camada de texto que o leitor pediu, os tiles da lista de demanda e, por último,
o trabalho de segundo plano. Este nunca toma o último worker ocioso, a menos que só haja um; nunca usa o último
quarto dos slots, que fica para tiles; e uma miniatura em andamento é cancelada assim que um tile visível precisa
do worker, voltando para a fila. Um trabalho que derruba workers duas vezes é abandonado e relatado, como um tile.
Os testes de sequências aleatórias da Parte 1 agora incluem esse trabalho, e a conservação dos slots continua
valendo.

### D4. Miniaturas: um tile, codificado pelo host, em arquivos pelo hash

A miniatura de uma página é o tile `(0, 0)` na maior escala em que a página cabe num quadrado de 256 pixels:
nenhum código de render novo. O worker entrega pixels, e o **host** codifica o JPEG (qualidade 82), para que o
que vai para o disco, e é decodificado depois, nunca seja um arquivo escrito por um processo comprometido.

Cada miniatura é um arquivo, `cache\thumbnails\v1\ab\<hash>\<página>.jpg`: escrita atômica por renomeação,
achar as que faltam com uma listagem de pasta, apagar um documento com uma pasta. Só documentos da biblioteca,
com hash confirmado, ganham miniaturas, porque é o hash que as identifica. Na amostra real, 10,3 KB em média.

### D5. libarchive só com o que lê ZIP e RAR

Do libarchive 3.8.11 entram só os arquivos da leitura de ZIP (pelo diretório central), RAR e RAR5 e o núcleo que
eles usam. Não há código para escrever arquivos, extrair para disco ou rodar programas como filtro: o que não foi
compilado não pode ser explorado. O zlib vem do `libz-sys`, compilado do fonte. O `config.h` foi gerado pelo CMake
com tudo desligado e está em `vendor/libarchive`, com a origem, o hash e a assinatura conferidos.

RAR só pode ser escrito sem compressão nas fixtures: o compressor é proprietário. Os descompressores de RAR são
cobertos pelos testes do próprio libarchive, não pelos nossos. As fixtures RAR4 e RAR5 foram conferidas pelo
7-Zip, que não compartilha código com o libarchive.

### D6. Páginas de quadrinho

- **Quais arquivos são páginas.** Imagens por extensão, menos os ocultos e as pastas `__MACOSX`; em ordem natural
  dos nomes, de modo que `p10` vem depois de `p9`.
- **Tamanho.** Lido do cabeçalho da imagem, com a orientação EXIF aplicada, sem decodificar a imagem. Cada pixel
  vale 0,24 ponto (300 dpi): um escaneamento de 1988 × 3056 tem o tamanho impresso de um gibi americano, e o modo
  contínuo o mostra como mostraria um PDF.
- **Direção.** `<Manga>Yes</Manga>` ou `YesAndRightToLeft` no `ComicInfo.xml` pedem leitura da direita para a
  esquerda, e as páginas duplas se espelham.
- **Pixels.** A transparência é composta sobre branco, como o papel. Cada tile é reamostrado da página
  decodificada só na região que cobre, e o filtro alcança além da borda do tile, então tiles desenhados um a um
  são iguais à página desenhada inteira: diferença máxima de 1 nível em dois fatores de escala testados.
- **Falhas.** Uma página danificada fica no lugar, com o tamanho da primeira página legível, e só ela falha.

### D7. DjVuLibre atrás de uma ponte

A API C do DjVuLibre é dirigida por mensagens, com macros e uma união de estruturas. Uma ponte em C++
(`crates/musaeum-djvu/shim`) a transforma em chamadas que terminam antes de voltar, e o lado Rust declara meia
dúzia de funções. Só documentos embalados (bundled) abrem: um documento indireto guarda as páginas em outros
arquivos, e o worker não tem como, nem permissão para, abri-los. Os bytes chegam pela API de stream, a partir do
mapeamento que o host compartilhou; a biblioteca nunca recebe um caminho.

As fixtures são escritas pelos codificadores do próprio DjVuLibre (IW44 para a cor, JB2 para o preto, BZZ para o
texto), como fazem `c44`, `cjb2`, `djvused` e `djvm`. Elas revelaram uma regra do formato: o DjVuLibre só compõe
uma página com camada colorida se a camada preta tiver cor própria (paleta `FGbz`); sem ela, desenha só o preto.
A ressalva: escrever e ler com a mesma biblioteca não prova compatibilidade com outros leitores de DjVu.

### D8. A política de sandbox de cada formato

Os workers de quadrinhos e DjVu não carregam DLL de terceiros, então rodam na política `LockdownParser`: o
sandbox inteiro, sem win32k e só com DLLs assinadas pela Microsoft. O de PDF continua em `Lockdown`, porque o
`pdfium.dll` usa o user32. O worker de DjVu importa o runtime C++ da Microsoft (`MSVCP140.dll`,
`VCRUNTIME140_1.dll`), assinado e portanto aceito; o instalador precisa garantir que ele esteja presente.

Pôr workers nessa política expôs uma falha: o pool só conferia que o worker largou o token de inicialização
quando a política era exatamente `Lockdown`. As duas políticas com sandbox começam pelo mesmo token, e a
verificação agora vale para as duas.

### D9. O formato pelos bytes

O Core lê os primeiros 1024 bytes do arquivo para escolher o worker, sem interpretar nada: `%PDF-` em qualquer
ponto deles (o PDF permite lixo antes), as assinaturas de ZIP e RAR, ou `AT&TFORM`. Um CBR que é um ZIP, comum na
prática, abre com o mesmo worker. `--document` substitui `--pdf`, que continua aceito.

### D10. A camada de texto

`ExtractText` devolve as palavras com suas caixas, como frações da página como ela é mostrada, com a rotação
aplicada, e o fim de cada linha. O PDF usa as caixas folgadas do PDFium; o DjVu, as zonas de palavra e de linha
da camada de texto. O formato no slot (`MTXT`) é validado inteiro pelo host antes de virar texto. No DjVu
girado, o centro de cada caixa cai sobre tinta preta na página desenhada, o que confere que texto e imagem giram
juntos.

### D11. A janela real pelo protocolo de depuração

O E2E (`tools/e2e-reader.mjs`) liga a porta de depuração do WebView2 só para a execução e conduz a página por
ela: cliques e teclas entram pelo pipeline de entrada do Chromium como eventos confiáveis, a página se comporta
como focada por emulação, e as capturas são da página, não da tela. Nada move o mouse ou manda teclas para a
janela que estiver em foco, o que importa numa máquina em uso. A janela ainda aparece, então a ferramenta só
começa depois de um minuto sem ninguém no computador, e confere de novo antes de cada documento. Com `--e2e`, a
página expõe o estado do leitor e, por uma mensagem ao compositor, o Locator exato e onde cada página está.

O pedido falava em atalhos para trocar o modo, e eles não existiam: `C` alterna entre paginado e contínuo, como no
SumatraPDF, e `A` liga e desliga a animação, que só existe no paginado, como o controle da barra.

As primeiras execuções acharam dois defeitos da própria ferramenta. O compositor descarta o que recebe antes de
terminar de iniciar, e a página expunha o estado antes disso: a primeira pergunta do teste se perdia e ele esperou
para sempre. A página agora só o expõe depois do aviso de pronto, e toda chamada ao protocolo tem prazo de 30 s.
E a inspeção misturava o leitor ao vivo com o último quadro desenhado: logo depois de uma virada sem animação, ela
dizia "parado e nítido" com a página anterior na tela. Ela agora diz se há um quadro por desenhar, e o teste espera
por ele.

### D12. Comparação visual contra a página inteira

A referência é a página desenhada inteira, de uma vez, pela biblioteca do formato: para o PDF, sem tiles e sem o
renderizador progressivo. A comparação é com a captura da janela real, pixel a pixel, deixando de fora a última
linha e coluna da página, que se misturam ao fundo. Assim ela confere tiles, compositor e WebView juntos.
`tools/visual-check` roda as bibliotecas no próprio processo, sem sandbox: é ferramenta de desenvolvimento, nunca
vai no app, e só lê as fixtures e uma amostra dos livros do usuário.

### D13. Cores

RGB tem uma resposta certa: os mesmos números. CMYK não tem sem um perfil, e o PDFium não faz gerência de cor:
converte `DeviceCMYK` com uma tabela fixa. As referências das primárias são o que o perfil U.S. Web Coated (SWOP) v2
dá em sRGB, pelo Little CMS (<https://polotno.com/tools/cmyk-to-rgb>). Preto e misturas dependem da intenção de
renderização e são relatados como medidos. A sobreimpressão não é simulada pelo PDFium: a área de amarelo sobre
ciano com overprint fica amarela, onde o Acrobat com a visualização de sobreimpressão mostra verde.

### D14. Licenças

- **DjVuLibre**, GPL-2.0-or-later: o "or later" permite usá-la sob a GPL-3.0. O plano a aprovou; o verificador
  aceita essa licença só para o que é vendorizado.
- **libarchive**: BSD-2-Clause, e CC0-1.0 ou Apache-2.0 nos arquivos de BLAKE2.
- **`jpeg-encoder`**: MIT ou Apache-2.0, e IJG. O `deny.toml` aceita a IJG só para esse crate; a nota de crédito
  que ela pede já vem com o libjpeg do PDFium.

### D15. Testes que medem o código, não o build

Decodificar e reamostrar imagens sem otimização é dezenas de vezes mais lento, e os testes do worker de quadrinhos
mediam o build: miniatura em P50 de 361 ms em debug contra 6 ms em release. Os crates de imagem passaram a
compilar otimizados no perfil de desenvolvimento, e a meta de 1 s do "morte até o próximo tile" é cobrada no
build de release, que é o que vai para o usuário. A substituição do worker é cobrada em qualquer build.

### D16. Uma margem em volta de cada tile do PDF

A comparação dos tiles com a página inteira mostrou, em todas as linhas de texto da fixture, a letra que
atravessava a primeira emenda vertical desenhada fora do lugar: o PDFium posiciona diferente um glifo cuja
origem fica à esquerda ou acima do bitmap, e o glifo dividido entre dois tiles saía com as duas metades
desencontradas, até 131 níveis de diferença. Nas páginas escaneadas sobrava ainda uma linha fraca na última
coluna e na última linha de alguns tiles, onde o reamostrador de imagens trata a borda do bitmap de outro jeito.

Cada tile agora é desenhado num bitmap de rascunho com 64 pixels a mais à esquerda e acima, para a origem desses
glifos cair dentro dele, e 16 à direita e abaixo, para o filtro de imagens não chegar à borda; só a região do
tile vai para o slot. São um terço a mais de pixels por tile. Medido nos 6 livros da amostra, com e sem as
margens, nas mesmas condições, o tempo de render por tile variou menos que a própria variação da máquina em uso.
`tests/seams.rs` compõe os tiles do worker real e exige igualdade com a página inteira; sem as margens ele falha.

## 5. Métricas

### 5.1 Acervo real (somente leitura)

| | |
|---|---|
| Acervo | 157 arquivos, 71 pastas, 7,3 GiB; inalterado (tamanho, datas, atributos); o Windows atualizou a data de último acesso das 228 entradas, como na Parte 1 |
| PDFs classificados | 144, 83.001 páginas, 0 falhas, 0 falhas do Core |
| Páginas | 71.750 texto (86,4%), 4.280 escaneadas (5,2%), 6.971 híbridas (8,4%) |
| Documentos | Pela regra de 95% (decisão da auditoria): 114 texto, 10 escaneados, 14 híbridos, 6 mistos, 55 estritamente uniformes. Pela regra estrita da medição: 40, 9, 6 e 89 (D2) |
| Tempo de classificação | 330 s no total, com 4 workers. Por página, conforme o documento, P50 de 0,06 a 159 ms; mediana dos P50 de 4,1 ms e dos P95 de 10,3 ms. O mais lento, um dicionário japonês escaneado com OCR (867 páginas, P50 de 159 ms), levou 38 s; a página mais lenta do acervo, 1,08 s |
| Miniaturas da amostra | 8.028 de 12 livros, 81 MB, 10,3 KB em média; P50 por livro entre 3,5 e 110 ms (escaneamentos pesados), máximo de 1,14 s; codificação no host em P50 de 0,8 a 1 ms |

### 5.2 Recuperação dos workers novos (release)

Os três na mesma rodada, com 8 mortes cada, em release:

| Worker | Substituição P50 / máx. | Morte → tile seguinte P50 / máx. |
|---|---|---|
| Quadrinhos, CBZ | 23,5 / 35,3 ms | 36 / 41 ms |
| Quadrinhos, CBR (RAR5) | 22,3 / 28,7 ms | 35 / 42 ms |
| DjVu | 24,5 / 30,1 ms | 39 / 51 ms |

Uma rodada anterior, só dos quadrinhos, deu 18 / 23 ms e 27 / 32 ms. Miniatura de quadrinho: P50 de 5,8 ms.

### 5.3 Tiles do pool contra a página inteira

`visual-check tiles`: os workers reais, no sandbox, desenham todos os tiles de cada página na escala em que ela
tem 1600 pixels de largura; os tiles são postos juntos e comparados com a página desenhada inteira, de uma vez,
pela biblioteca do formato. Diferença em níveis de 0 a 255, no pior canal.

| Documento | Páginas | Idênticas | Maior diferença | Pixels com diferença acima de 2 |
|---|---|---|---|---|
| `content.pdf` (texto, escaneada, híbrida, form XObject, girada) | 8 | 8 | 0 | 0% |
| `mixed.pdf` (tamanhos, `/Rotate`, CropBox) | 6 | 6 | 0 | 0% |
| `colors.pdf` | 1 | 1 | 0 | 0% |
| `comic.cbz`, `comic-rar5.cbr` | 6 | 3 | 1 | 0% |
| `book.djvu` | 5 | 1 | 2 | 0% |
| Acervo: 6 livros (escaneados pesados, misto, o mais longo, comuns) | 12 | 9 | 9 | 0,25% numa página, 0% nas outras |

Nos quadrinhos e no DjVu, a diferença de 1 a 2 níveis é arredondamento do reamostrador. Na página do acervo com
9 níveis, as diferenças seguem o contorno de uma ilustração, não as emendas; a causa não foi investigada.

### 5.4 Cores do PDFium

| Retalho | PDFium | Referência | Diferença |
|---|---|---|---|
| RGB vermelho, verde, azul, cinza 50%, cinza 25%, laranja | iguais | os mesmos números | 0 |
| CMYK ciano | 0, 174, 239 | 0, 174, 239 | ΔE00 0 |
| CMYK magenta | 237, 2, 140 | 236, 0, 140 | ΔE00 0,28 |
| CMYK amarelo | 255, 241, 1 | 255, 242, 0 | ΔE00 0,28 |
| CMYK branco | 255, 255, 255 | 255, 255, 255 | 0 |
| CMYK preto, preto rico | 35, 31, 32; 0, 0, 1 | depende da intenção | medido |
| CMYK vermelho, verde, azul | 238, 29, 35; 0, 165, 79; 46, 48, 146 | depende da intenção | medido |
| Amarelo sobreimpresso em ciano | 255, 241, 1 (amarelo) | verde, se simulado | não simulado |

Preto e misturas diferem em no máximo 2 níveis por canal dos valores que o Adobe Illustrator costuma mostrar para
essas cores (`#231F20`, `#ED1C24`, `#00A651`, `#2E3192`), mas não encontrei uma fonte publicada que confirme
esses valores.

### 5.5 Na janela real

**Benchmark de zoom refeito.** O cenário da Parte 1 (`--scenario zoom`, janela de 1600 × 900, caminho C), nos
mesmos dois livros, no build final, com a máquina parada: 5 minutos sem uso, GPU a 5%, nenhum jogo aberto. Os 6
fatores de zoom de cada livro, 12 no total:

| Medida | Parte 1, antes dos ajustes | Build final |
|---|---|---|
| Até nítido depois de parar | 13,1 a 13,5 ms, 12 de 12 | 13,3 a 13,4 ms, 12 de 12 |
| Quadros em branco | 0 | 0 |
| Trabalho por quadro | P99 2,2 ms, máximo 3,5 ms | P99 2,2 ms, máximo 3,9 ms |
| Quadros com substituto por zoom | 22 a 28 | 22 a 28 |
| Despejos, orçamento de 80 MiB | 52 a 83 | 55 a 85 |

A tela continua nítida no quadro seguinte ao fim da animação, agora com a margem dos tiles (D16), que custa um
terço a mais de pixels por tile. O ADR 0003 foi atualizado com estes números.

**Posição de leitura.** `tools/e2e-reader.mjs`, no build de release, com o computador parado, em 73 s para tudo
desta seção menos o zoom. Em cada formato, numa biblioteca nova numa pasta temporária, o teste desce para dentro
do documento no contínuo, para que v não seja um número redondo; troca para paginado e desliga a animação pela
barra; alterna com `C` para contínuo e de volta, com a animação ainda desligada; religa com `A`; vira uma página
com a animação, desliga com `A` e vira outra; volta ao contínuo pela barra; fecha a janela como o botão de fechar
faria; e abre o documento de novo.

| | PDF (`content.pdf`) | Quadrinho (`comic.cbz`) | DjVu (`book.djvu`) |
|---|---|---|---|
| Passos | 13 de 13 | 13 de 13 | 13 de 13 |
| Locator antes das trocas | página 0, u 0, v 0,7759303246239112 | página 2, u 0, v 0,9135802469135799 | página 1, u 0, v 0,15123456790123438 |
| Trocas de modo ou de animação com o Locator igual em todos os dígitos | 7 de 7 | 7 de 7 | 7 de 7 |
| Virar com animação / sem | deslizou / troca seca | deslizou / troca seca | deslizou / troca seca |
| Reaberto: o que o `library.db` devolveu e o que a janela mostrou | iguais ao salvo nos 8 campos | iguais | iguais |

**Janela contra a página inteira.** A captura é da própria página do WebView, com a barra de leitura escondida,
no paginado com uma página por tela, e cada página é comparada com a mesma página desenhada inteira pela biblioteca
do formato, na escala exata em que a janela a mostra. Os livros do acervo são os 6 da seção 5.3, abertos só para
leitura numa biblioteca vazia.

| Documento | Páginas | Idênticas | Maior diferença | Pixels acima de 2 níveis |
|---|---|---|---|---|
| PDFs das fixtures (`content.pdf`, `colors.pdf`, `mixed.pdf`) | 8 | 8 | 0 | 0% |
| `comic.cbz` | 4 | 4 | 0 | 0% |
| `book.djvu` | 3 | 0 | 3 | 0,002% numa página, 0% nas outras |
| 6 livros do acervo | 12 | 10 | 1 | 0% |

As diferenças são de arredondamento: no DjVu, da reamostragem, como na seção 5.3; no acervo, nas duas páginas do
livro de Biologia. A página desse livro que tinha 9 níveis entre os tiles e a página inteira, a 1600 pixels de
largura, tem 1 nível na escala da janela.

**Cores na janela.** Os 17 retalhos de `colors.pdf` saem na janela com exatamente os números que o PDFium desenha:
diferença 0 em todos os canais. O compositor e o WebView não mudam a cor. Medido com o WebView em sRGB
(`--force-color-profile=srgb`); com o perfil do monitor, o Chromium converteria as cores para ele, o que é o
esperado e não foi medido.

## 6. Limitações conhecidas

- **Não há quadrinho nem DjVu no acervo.** Os dois formatos foram medidos só com fixtures geradas aqui.
- **RAR comprimido** não tem fixture própria (D5). Um RAR sólido custa, para abrir uma página, descomprimir as
  anteriores.
- **Camadas JPEG em DjVu** (`BGjp`, `FGjp`) não são desenhadas, e documentos DjVu indiretos não abrem (D7).
- **ZIP com bzip2, LZMA, XZ ou zstd** não abre; deflate, PPMd e sem compressão abrem.
- **Sobreimpressão** não é simulada pelo PDFium (D13).
- **Medições com a máquina em uso** (seção 3): tempos das seções 5.1 a 5.4 são piores que os de uma máquina
  parada.
- **Glifos mais largos que a margem.** Uma letra com mais de 64 pixels à esquerda da emenda, possível em títulos
  com muito zoom, ainda pode sair deslocada (D16).
- **Um teste intermitente da Fase 1.** `a_book_that_comes_back_as_a_placeholder_is_recognized_once_it_is_local`
  (`musaeum-library/tests/cloud.rs`) falhou uma vez em 50 execuções do binário: o Windows recusou registrar a
  raiz de sincronização com `0x80070490` (elemento não encontrado). As outras 49 passaram, inclusive 40 seguidas.
  Os nove testes do binário registram raízes do mesmo provedor em paralelo, e uma disputa no serviço Cloud Files
  é a hipótese, não comprovada. Na auditoria, o projeto decidiu serializar esses registros (seção 8).

**Itens da seção 6 do ADR 0003 que esta parte fecha:** a classificação texto, escaneado ou híbrido e as
miniaturas em disco. O modo paginado passou a ser exercitado no app, pelo E2E (seção 5.5), mas não medido em
tempo; trackpad e roda físicos continuam sem teste.

## 7. Como reproduzir

```powershell
cargo test --workspace
cargo build --release -p corpus-check -p musaeum-render-pdf -p musaeum-library --bins
.\target\release\corpus-check.exe --corpus D:\Livros --work D:\Temp\verificacao --sample 12
cargo test --release -p musaeum-render-raster -p musaeum-render-djvu -- --nocapture
.\target\release\gen-fixtures.exe --out fixtures
.\target\release\visual-check.exe colors --document fixtures\colors.pdf
.\target\release\visual-check.exe tiles --document fixtures\content.pdf --pages 0,2,5
node tools\e2e-reader.mjs --corpus-sample amostra.txt
```

## 8. Decisões da auditoria (2026-11-03)

O projeto auditou a Parte 2 e aprovou o fecho da Fase 2: os 39 passos do E2E na janela real, a comparação visual das
27 páginas e o benchmark final de zoom. Três decisões ficaram registradas:

1. **Resumo por maioria qualificada.** `document_geometry.content` é a classe de pelo menos 95% das páginas
   (`CONTENT_MAJORITY_PERCENT`), para que livros de texto com capa ou pranchas escaneadas não fiquem todos como
   `mixed`. A coluna nova `is_strictly_uniform` guarda se todas as páginas têm a mesma classe. Com 95% exatos, a
   classe vale; por isso um documento de menos de 20 páginas só tem classe se todas forem iguais. A migração 0003
   ainda não tinha saído desta máquina, então a coluna entrou nela, sem migração nova. O efeito no acervo está em D2.
2. **Aprovados como estão:** os atalhos `C` (alternar o modo) e `A` (alternar a animação), a margem de 64 px à
   esquerda e em cima e 16 px à direita e embaixo dos tiles de PDF (D16), e a exigência das DLLs do runtime C++ da
   Microsoft (MSVCP140 e VCRUNTIME140_1) pelo worker de DjVu (D8).
3. **Registro de raízes serializado em `tests/cloud.rs`.** Um mutex do teste envolve o registro e a conexão de
   cada raiz, e a desconexão e a remoção, de modo que essas chamadas à Cloud Filter API não se sobrepõem mais; os
   testes continuam rodando em paralelo. Depois da mudança, o binário rodou 100 vezes
   seguidas sem falha. Como a falha era de 1 em 50, isso é indício, não prova: com aquela taxa, 100 rodadas limpas
   ainda aconteceriam em cerca de 13% das vezes.
