# ADR 0006: decisões da Fase 3, Parte 2, e do marco Alpha

| | |
|---|---|
| **Data** | 2026-11-09 |
| **Situação** | Decisões aceitas. O projeto auditou e aprovou a Parte 2 e o marco Alpha, com as decisões da seção 9 |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo, WebView2 157 |
| **Escopo** | O leitor fluido no app (paginado e rolagem contínua própria, tipografia), o contrato único de leitura para livros fixos e fluidos com a posição na biblioteca, e a superfície de biblioteca do marco Alpha |

## 1. Resumo

O Musaeum passa a ser um programa de uso diário: abre na biblioteca, que segue as pastas escolhidas e
lista os livros em capas ou em lista, com busca instantânea e filtro por pasta; um clique abre o livro no
leitor certo para o formato, e uma tecla volta à biblioteca sem fechar o programa. Os livros fluidos abrem
pelo pacote que o normalizador da Parte 1 guarda no cache, numa origem própria, sem IPC.

Nos 13 EPUBs reais do acervo, o leitor manteve a âncora no mesmo caractere, e no lugar, em 195 de 195
mudanças de tipografia, tela e modo; reabriu no mesmo caractere em 26 de 26 vezes; e pôs o texto depois
de cada um dos cortes do normalizador exatamente onde o documento inteiro o põe (81 emendas, 0,000 px).
Chegar a isso custou três correções que os fixtures não pediam (seção 6): as emendas somavam margens,
algumas seções perdiam os estilos do livro por uma corrida no carregador do foliate-js, e o console do
leitor tinha quatro erros por livro.

Na janela real, com o build final, as duas passadas terminaram sem falhas. Nas fixtures, os nove livros
fluidos mantiveram a âncora em 135 de 135 mudanças e reabriram pelo `library.db` no mesmo caractere em 18
de 18 vezes, e os fixos reabriram na página exata. No acervo, os 13 EPUBs e os 144 PDFs abriram pela
biblioteca, e o acervo saiu igual (seção 5).

## 2. Entregas

| Pedido | Onde | Como foi conferido |
|---|---|---|
| Pacotes normalizados no leitor, em `book.<aleatório>.localhost`, CSP estrita, sem IPC | `src-tauri/src/book.rs`, `flow.rs`, `reader/package-loader.ts` (D2) | Testes da política e do servidor do pacote; no livro hostil, nenhum script do livro rodou |
| Paginado fluido, virada animada por padrão e instantânea como opção | `reader/paginated-view.ts`, tecla A | E2E: a virada leva ao primeiro caractere da página seguinte, com e sem animação |
| Rolagem contínua virtualizada própria, sem saltos entre partes de 200 KiB | `reader/continuous-view.ts` (D4) | 81 emendas a 0,000 px do documento inteiro; linha da âncora a até 0,49 px do topo |
| Tipografia (família, tamanho, entrelinha, margens, claro, sépia, escuro) com crossfade | `reader/typography.ts`, `flow-engine.ts` (D5) | As 15 mudanças do portão em cada livro |
| Contrato único fixo/fluido: barra, sumário, progresso, C e A, `reading_state` | `app/reader/ReaderView.tsx`, `fixed/fixed-reader.ts`, migração 0003 (D6, D7, D11, D13) | E2E com PDF, CBZ, DjVu e os nove livros fluidos pela mesma barra |
| Locator a até uma linha ao mudar fonte, margens, janela ou modo | D3, D6 | 195/195 nos EPUBs reais sem janela, 135/135 nos fixtures na janela |
| Fechar e reabrir no ponto exato | D6, D13 | 18/18 nos fixtures e 13/13 no acervo, na janela, pelo `library.db`; 26/26 pelo leitor nos EPUBs reais sem janela; PDF, CBZ e DjVu na mesma página |
| Alpha: biblioteca com pasta raiz ou existente, grade e tabela virtualizadas com capas, filtro de pasta, busca FTS5, clique abre, volta sem fechar | `app/library/`, `src-tauri/src/shelf.rs`, `covers.rs`, `catalog.rs` (D8, D9, D10) | E2E com as fixtures: listagem, busca, pastas, capas, abrir e voltar |
| E2E na janela com os 13 EPUBs e os PDFs do acervo, só lendo | `tools/e2e-alpha.mjs` | 13/13 EPUBs e 144/144 PDFs abertos pela biblioteca, com posição e emendas conferidas; acervo igual antes e depois (5.3) |

## 3. Como foi medido

- **Na janela real, pelo CDP.** `tools/e2e-alpha.mjs` abre o app de verdade, com uma biblioteca numa pasta
  temporária, e o conduz pelo protocolo de depuração do próprio WebView2. Cliques e teclas entram como
  input confiável pela página (o mesmo caminho de um mouse), sem mover o mouse do sistema nem mandar teclas
  para a janela em foco. A janela é redimensionada por um comando que só existe com `--e2e`. O iframe do
  livro não aparece como alvo próprio do CDP no WebView2; o teste o alcança pelos contextos de execução da
  página, escolhendo o do `reader.html` (as seções são documentos `blob:` da mesma origem).
- **A âncora, por dentro.** Com `--e2e`, o leitor expõe a âncora (o CFI), onde o caractere dela é
  desenhado e a altura da linha. O teste compara a âncora antes e depois de cada mudança como texto exato,
  e confere a posição na tela.
- **Só com a máquina livre.** Uma rotina espera o computador ficar parado (sem input, sem jogo aberto, GPU
  e CPU baixas) e o teste para sozinho se alguém mexer. As capturas são da própria página, nunca da tela.
- **O acervo só é lido.** A fase do acervo lista tamanho, datas e atributos de cada arquivo e pasta antes
  e depois, e falha se algo mudar; biblioteca e cache ficam numa pasta temporária, apagada no fim.
- **O leitor sem janela, nos livros reais.** `tools/reader-check.mjs` serve o `reader.html` construído, com
  a mesma política do app, e os pacotes normalizados de um cache, em duas origens locais (uma faz o papel
  da página principal e fala com o leitor pelas mensagens de `messages.ts`), e o conduz num Chrome
  headless, com perfil temporário e sem GPU: não abre janela, não usa input e pode rodar com a máquina em
  uso, em prioridade baixa. Em cada livro, o mesmo portão de posição da janela (15 mudanças), a virada de
  página, a reabertura exata nos dois modos, todas as emendas e os erros do console.
- **A emenda, contra o documento inteiro.** Comparar a emenda com o espaço "típico" entre parágrafos não
  serve antes de um título, e os parágrafos dos fixtures não têm margem. `tools/lib/seam-oracle.mjs` junta
  de novo as duas partes num documento só (a primeira inteira, e o conteúdo da segunda dentro dos mesmos
  contêineres), o desenha num iframe escondido da mesma largura e mede onde cai a primeira caixa da
  segunda parte. O leitor tem de pôr essa caixa no mesmo lugar, com tolerância de 1 px. Os dois testes, o
  da janela e o headless, usam esse oráculo.

## 4. Decisões

### D1. React e React Aria na página principal; o leitor continua sem framework

A biblioteca e a barra de leitura usam React 19 e React Aria Components, como o plano previa: a grade e a
tabela virtualizadas, a árvore de pastas, o campo de busca, os grupos de opção e o painel de tipografia
vêm prontos com teclado e leitores de tela. Isso fica num pedaço do bundle (cerca de 590 kB, 177 kB com
gzip) que só a página principal carrega, por `import()`, e só quando o programa abre como app. O iframe do
leitor, na origem do livro, não carrega React nem nada da interface: o código que convive com o livro
continua pequeno. A página das fases anteriores (benchmarks e testes com `--document` e `--epub`)
continua a mesma.

### D2. O livro normalizado é servido arquivo por arquivo, do cache, na origem do livro

Abrir um livro fluido normaliza o arquivo no worker isolado, se o cache ainda não tiver aquela versão dos
bytes, e registra uma origem nova, `http://book.<128 bits aleatórios>.localhost`, que serve:

| Caminho | Conteúdo | Política |
|---|---|---|
| `/reader.html` | O leitor | A da Fase 0, com nonce |
| `/assets/…` | Os scripts do leitor | — |
| `/pkg/…` | Os arquivos do pacote, lidos do cache | A mesma, com `script-src 'none'` |

O foliate-js lê o pacote por um carregador HTTP (`package-loader.ts`): sem ZIP, sem cópia do livro em
memória. O caminho pedido passa pela mesma checagem que o cache aplicou ao gravar (relativo, poucos
segmentos, nada que suba de pasta). O iframe do leitor tem `allow-scripts allow-same-origin`, as seções
têm só `allow-same-origin`, e a origem do livro não tem capacidade nenhuma no Tauri: a página só recebe
dele as mensagens de `messages.ts`, conferidas campo a campo.

### D3. Uma âncora por livro: o primeiro caractere que o leitor vê

A posição de leitura é um caractere, guardado como CFI do leitor: no paginado, o primeiro caractere da
página mostrada; no contínuo, o primeiro da linha no topo da tela (uma linha cortada pela borda conta). Só
virar a página, rolar ou ir a algum lugar move a âncora. Mudar fonte, tamanho, entrelinha, margens, tema
ou janela refaz o layout em volta dela, e trocar de modo abre a outra vista nela.

No contínuo, "manter a posição com margem de uma linha" é medido literalmente: depois de cada mudança, a
linha da âncora volta ao topo da tela. No paginado, as quebras de página são do layout, não do leitor: a
página mostrada é a que contém a âncora, e a âncora continua sendo o mesmo caractere. A auditoria aprovou
isso (seção 9). A alternativa, recompor as colunas para que a página comece exatamente na âncora,
desalinharia todas as páginas seguintes.

Como a âncora não muda com o layout, ela não deriva: depois de vinte mudanças seguidas, a posição salva
é a mesma da primeira.

### D4. Rolagem contínua própria, virtualizada

O foliate-js rola uma seção por vez. A vista contínua (`continuous-view.ts`) põe todas as seções numa só
rolagem, cada uma num iframe da altura do seu texto, de modo que a rolagem é a do navegador, suave e
nativa, de uma seção para a outra.

- **Virtualização.** Ficam carregadas as seções de uma tela acima a duas abaixo; as que passam de quatro
  telas acima ou cinco abaixo saem, guardando a altura medida.
- **Alturas.** Uma seção nunca carregada tem a altura estimada pelo comprimento do texto, com a razão
  pixels por caractere aprendida das seções já medidas. Quando uma seção acima da tela carrega e a altura
  real difere, a rolagem anda a diferença no mesmo quadro: nada na tela se mexe.
- **Costuras.** Onde o normalizador cortou um documento em partes, os contêineres que ele repetiu perdem
  margem, preenchimento e borda do lado do corte; os elementos do livro guardam os seus. Num documento só,
  a margem de baixo do último bloco antes do corte e a de cima do primeiro depois se fundem na maior das
  duas; em dois iframes, elas se somariam. Por isso a parte de cima ocupa na coluna só até onde o seu
  fluxo de blocos termina, menos a menor das duas margens, e o iframe dela continua por baixo da parte
  seguinte. Onde o fluxo termina é o navegador que diz: um elemento vazio, com borda em cima e sem
  margem, posto depois do último bloco da parte, fica logo abaixo da margem desse bloco, sem contar os
  floats. Um float que passa do fim do fluxo continua desenhado no iframe de cima. O capítulo segue como
  se não tivesse sido cortado, e a seção 3 diz como isso é conferido.
- **Âncora.** O caractere é achado por busca binária nos retângulos do texto; ao mudar a tipografia, todas
  as seções carregadas são medidas de uma vez e a rolagem volta a pôr a linha da âncora no topo.

### D5. Crossfade pela View Transitions API, no iframe do leitor

Uma mudança de tipografia, de tema ou de modo roda dentro de `document.startViewTransition`, no documento
do leitor: a imagem antiga se dissolve na nova em 220 ms, e o relayout (que leva de 20 a 70 ms, ADR 0005)
fica escondido atrás dela. Tudo o que o relayout precisa acontece de forma síncrona dentro da transição,
porque o navegador suspende o desenho enquanto ela prepara a imagem nova. Uma mudança que chega no meio
da anterior corta a anterior, o que é esperado. Com "reduzir movimento" no sistema, não há transição.

### D6. A posição vai para a biblioteca como Locator, calculado no Rust

O leitor só conhece o seu CFI, que conta nas partes. Meio segundo depois de a âncora parar, e de novo ao
fechar o livro, a página manda esse CFI ao host, que o converte em CFI canônico, acha o ponto no
documento inteiro (montado das partes, com cache dos últimos quatro documentos) e grava o Locator
completo: CFI canônico, progressões e o texto citado em volta. Ao abrir, o host acha o Locator de novo
(CFI, depois a citação, depois a progressão) e devolve o CFI do leitor. É a mesma aritmética validada na
Parte 1 em 2,1 milhões de posições.

### D7. `reading_state` guarda os dois tipos de livro

A migração 0003 da biblioteca acrescenta `reading_state.locator` (JSON, conferido por `json_valid`) e a
tabela `preference`, para a tipografia, que vale para todos os livros. Um livro fluido usa a mesma linha
da tabela, com a página 0 em (0, 0), zoom 1 e páginas duplas automáticas, que não significam nada para
ele, e o modo e a animação nas colunas de sempre. A barra, os atalhos e a persistência são os mesmos para
os dois tipos.

### D8. A biblioteca do app

A biblioteca aberta é a do `--library` ou a padrão, se já existir; uma nova só é criada quando a pessoa
escolhe uma pasta de livros, e nunca dentro dela. Com a biblioteca aberta, o indexador da Fase 1 segue as
pastas e a página ouve os eventos dele. A listagem traz um livro por documento, pelo melhor arquivo dele
(presente antes de só na nuvem, a revisão mais nova), por título ou pelo ranking da busca FTS5, numa pasta
e nas subpastas, em páginas de 400. O seletor de pasta é o do sistema (`IFileOpenDialog`), aberto pelo
host: a página só recebe o caminho escolhido.

### D9. TXT e Markdown entram na biblioteca

Para guardar a posição de livros em TXT e Markdown, como pedido, o indexador passa a reconhecê-los. Para
não varrer as notas de software que costumam estar em qualquer pasta, `readme.*`, `license.*`,
`licence.*` e `changelog.*` ficam de fora por padrão.

### D10. Nenhuma imagem de um livro é decodificada na página principal

A página principal tem o IPC; as imagens de um livro só são decodificadas em processos isolados. A capa de
um livro fixo é a primeira página, desenhada pelo worker do formato, como na leitura. A de um livro
fluido é a imagem que o livro nomeia: o normalizador a acha no worker `ingest` (tarefa nova, `Cover`), o
host confere pelos primeiros bytes que é JPEG, PNG, GIF ou WebP, e o worker de quadrinhos a desenha, como
a página única de um CBZ feito na hora. O que chega à página é um JPEG de 256 px codificado pelo próprio
programa, guardado com as miniaturas, pelo hash do arquivo, e servido por `thumb://`. Uma capa que não
decodifica, ou um livro sem capa, fica marcado e mostra uma capa tipográfica.

### D11. O sumário dos PDFs, pelos marcadores

O sumário de um PDF vem dos marcadores, lidos pelo PDFium no worker, com profundidade e quantidade
limitadas e proteção contra marcadores que apontam para si mesmos. Ele viaja pelo caminho já conferido
da camada de texto: é a "camada de texto" de uma página que nenhum documento tem, `OUTLINE_PAGE`, com
nível, página e título em cada palavra; o host confere níveis e páginas contra o documento. O protocolo
dos workers sobe para a versão 4. Quadrinhos não têm sumário; o de DjVu ainda não é lido.

### D12. Links para fora do livro

O leitor intercepta o clique num link `http(s)`, `mailto` ou `tel` (ADR 0005, seção 8) e a página mostra o
endereço, com dois botões: **Copiar endereço** e, para `http` e `https`, **Abrir no navegador**. O programa
não abre o navegador sozinho; só o botão, apertado pelo leitor, o faz. O endereço vem do livro, então o
Rust o confere de novo antes de entregá-lo ao `ShellExecuteW` (`src-tauri/src/links.rs`): só `http` e
`https`, ASCII imprimível, sem espaços, aspas nem barra invertida, sem usuário e senha no endereço, e um
host de rótulos válidos ou um IPv6 entre colchetes. `mailto` e `tel` só se copiam. Com `--e2e`, o comando
anota o endereço em vez de abrir o navegador, e o teste na janela confere que o diálogo mostra o
endereço, que nada abre antes do botão e que o botão pede exatamente aquele endereço. Essa forma foi a
decisão da auditoria (seção 9).

### D13. O leitor fixo virou um módulo

O código do leitor fixo saiu da página das fases anteriores para `fixed/fixed-reader.ts`, que abre e fecha
um documento num canvas novo; a página antiga e o app usam o mesmo módulo. Ao fechar, o compositor manda
a posição que ainda esperava a pausa de 400 ms (mensagem nova, `flush`), e o host a grava antes de parar
os workers: fechar e reabrir volta ao ponto exato.

## 5. Resultados

### 5.1 O leitor sem janela, nos 13 EPUBs reais

`tools/reader-check.mjs` nos 13 EPUBs do acervo, normalizados pelo `corpus-check --flow-only` da Parte 1
(o acervo só foi lido), num Chrome headless 156, janela de 1400 × 900, com o computador em uso e o
teste em prioridade ociosa. Em cada livro: vai a 37 % do texto, faz as 15 mudanças do portão (tamanho,
entrelinha, margens, família, tema, duas telas, paginado ↔ contínuo), vira a página com a animação
desligada, reabre na posição virada (paginado) e depois de rolar (contínuo), e confere cada emenda, de
novo com outra tipografia nas quatro primeiras.

| | Resultado |
|---|---|
| Mudanças com a âncora no mesmo caractere e no lugar | **195 de 195** (13 × 15) |
| Contínuo: maior distância da linha da âncora ao topo | 0,49 px |
| Página virada para o caractere seguinte | 13 de 13 |
| Reaberturas no mesmo caractere | **26 de 26** (paginado e contínuo) |
| Emendas medidas contra o documento inteiro | **81** em 7 livros (60 cortes, 21 de novo com serifa a 130 %, margens largas e entrelinha 1,35); maior diferença **0,000 px** |
| Cortes com colunas ou com floats passando do fim da parte | 0 |
| Arquivos do livro revogados enquanto em uso | 0 (antes da correção, 1 ou 2 por execução no *Netter*, no PBR e nas obras de Orwell, que nem tem cortes) |
| Erros no console do leitor | 0 em 39 aberturas (antes da correção, 4 por abertura: o preload sem nonce e três 404 de `META-INF`) |

Os tempos de abertura (52 a 180 ms do `ready` ao `opened`, no Chrome, com o pacote já no cache) foram
tomados com a máquina em uso, por um jogo em tela cheia e depois pelo E2E da janela, e não valem como
medida.

### 5.2 Na janela real, nas fixtures

`tools/e2e-alpha.mjs --phase fixtures`, passada 13, com o build final e a máquina ociosa: uma biblioteca
temporária com um livro de cada formato (13 livros em duas pastas), aberta, buscada e filtrada pela
interface; cada livro aberto por um clique no cartão.

| | Resultado |
|---|---|
| Biblioteca | 13 livros listados; busca ("hostile" 1, "mixed" 1, "book" 2); pastas `fluidos` 9 e `fixos` 4; 10 capas desenhadas e 3 tipográficas |
| Livros fluidos | 9: EPUB 3, EPUB 2, EPUB hostil, MOBI, AZW3, FB2 em UTF-8 e em cp1251, TXT, Markdown |
| Mudanças com a âncora no mesmo caractere e no lugar | **135 de 135** (9 × 15: tipografia pelo painel da barra, a janela redimensionada e o modo pela tecla C) |
| Contínuo: maior distância da linha da âncora ao topo | 0,50 px |
| Virada de página com a animação desligada (tecla A) | 9 de 9 |
| Fechar e reabrir no mesmo caractere, pelo `library.db` | **18 de 18** (paginado e contínuo, rolado com PageDown e seta) |
| Emendas, contra o documento inteiro | Δ = 0 px nos dois livros cortados (TXT e EPUB) |
| Livro hostil | nenhum script dele rodou em nenhuma seção |
| Relayout de tipografia, por trás do crossfade | 1 a 52 ms |
| Troca de modo (abre a outra vista na âncora) | 16 a 80 ms; 302 ms na primeira ida do livro hostil ao contínuo |
| Livros fixos | PDF, PDF com sumário (3 entradas; o salto chegou à página), CBZ e DjVu: abertos e nítidos em 104 a 117 ms, e reabertos na página exata |
| Erros no console da página | 0 |

Nos 13 EPUBs reais sem janela (5.1), com a máquina em uso, a maior troca de modo foi a do PBR (cerca de
470 ms para paginar a seção mais pesada) e o maior relayout de tipografia, 97 ms (*Real-Time Rendering*),
dentro da faixa da Parte 1; os mesmos números de antes da serialização das cargas de seção.

### 5.3 Na janela real, no acervo

`tools/e2e-alpha.mjs --phase corpus`, com o build final, a máquina ociosa (sem input desde antes do
início, GPU abaixo de 4 %) e uma biblioteca nova numa pasta temporária apontada para
`C:\Marte\Archives\DigitalLibrary\Books`. Cada livro abre pela busca e por um clique no cartão.

| | Resultado |
|---|---|
| Livros listados | 157 (13 EPUBs, 144 PDFs) |
| EPUBs abertos | **13 de 13**; a primeira abertura normaliza o livro no worker (de 0,3 s a 7,0 s, o maior no *Real-Time Rendering*); a reabertura, do cache, de 0,22 a 0,64 s |
| EPUBs: âncora mantida com a fonte maior | 13 de 13 |
| EPUBs: reabertura no mesmo ponto | **13 de 13** (pelo Locator do `library.db`) |
| EPUBs: primeira emenda, contra o documento inteiro | 7 livros cortados, Δ = 0 px em todos |
| PDFs abertos e nítidos | **144 de 144**, 83 001 páginas |
| PDFs: do clique ao leitor | mediana 114 ms, p95 224 ms, máximo 455 ms |
| PDFs: do leitor à primeira tela nítida | mediana 545 ms, p95 769 ms, máximo 3 062 ms |
| PDFs com sumário | 106; o salto pelo sumário chegou à página certa em 106 de 106 |
| Erros no console da página | 0 |
| Acervo antes e depois | **igual**: 228 entradas, 7 822 677 893 bytes, mesmos tamanhos, datas de criação e modificação e atributos |

Os tempos são do teste, pelo CDP, e incluem as idas e voltas dele; não substituem os benchmarks das
fases anteriores.

## 6. Defeitos achados no caminho

As passadas do E2E acharam estes defeitos, todos corrigidos antes das medidas finais:

| Onde | O que acontecia | A correção |
|---|---|---|
| Biblioteca | A coluna da grade (`1fr`) crescia até caber todas as capas numa linha; a página rolava na horizontal e o foco, ao rolar a grade, tirava o cartão de baixo do clique, que se perdia | `minmax(0, 1fr)` e `min-width: 0` |
| Paginado | Virar a última página de uma seção leva o foliate-js à seção seguinte com o motivo "navigation", que a vista tratava como uma navegação pedida por ela: a âncora ficava na seção anterior, e a posição salva também | Só a navegação que a própria vista inicia mantém a âncora; as outras a tiram da página mostrada |
| Paginado | Uma página que começa logo depois do espaço que termina a linha anterior era ancorada nesse espaço, que fica na coluna de antes | A âncora pula os espaços até o primeiro caractere visível |
| Barra de leitura | Depois de fechar o painel de tipografia, o foco volta ao botão "Aa", e as setas não viravam a página | Só controles que andam com as setas (grupos de opção, listas) ficam com elas; o espaço continua apertando o botão focado |
| Crossfade | Uma mudança no meio da anterior deixava no console a rejeição da transição cortada | As duas promessas são tratadas |
| Fixtures | O PNG de 1 pixel do `gen-fixtures` tinha o CRC do bloco IDAT errado; o worker de quadrinhos o recusava, como deve, e a capa ficava tipográfica | CRC corrigido |
| Contínuo, emendas | Nos livros reais cortados, as margens dos dois lados do corte se somavam: 9 + 21,6 px antes de um título em *Systems Performance*, 18 + 19,8 px no *Netter*, onde o documento inteiro tem 21,6 e 19,8 px. E o primeiro elemento do livro na parte seguinte perdia a borda e a margem de cima junto com os contêineres (um passo a mais no caminho do corte). Os fixtures não mostravam nada disso, porque os parágrafos deles não têm margem, e o teste comparava com o espaço entre parágrafos, que não vale antes de um título | As margens se fundem na maior (D4), só os contêineres perdem as bordas, e o teste usa o documento inteiro como referência (seção 3): as 81 emendas medidas nos livros reais ficam a 0,000 px dele |
| Contínuo, estilos | O carregador do foliate-js conta quem segura cada arquivo do livro, e a conta falha com seções carregando ao mesmo tempo, como a rolagem contínua faz: duas seções com a mesma folha de estilos a criavam duas vezes, a segunda substituía a primeira no cache com contagem 1, e soltar uma das seções revogava a folha da outra. Uma seção cujo iframe ainda não a tinha lido aparecia **sem os estilos do livro** (visto no *Netter* e nas *Obras completas* de Orwell, como `ERR_FILE_NOT_FOUND` de um `blob:` `text/css`). Uma seção pedida de novo enquanto carregada também era contada uma vez só e solta todas as vezes | `section-loads.ts`, no adaptador: as seções carregam uma depois da outra, e o foliate-js carrega cada uma uma vez enquanto alguém a segura e a solta com o último. O foliate-js vendorizado não mudou |
| Leitor | O `<link rel="modulepreload">` que o Vite põe no `reader.html` não recebia o nonce, e a política o recusava a cada livro aberto, com um erro no console e o módulo buscado de novo pelo `import` | O nonce vai também nos preloads de módulo |
| Leitor | O foliate-js pede três arquivos de `META-INF` que um livro pode ter (cifragem e opções de exibição da Apple e da Kobo); o normalizador só grava o `container.xml`, e cada livro aberto deixava três 404 no console | O carregador do pacote responde que esses arquivos não existem, sem pedir |

E estes, do próprio teste: o iframe do livro não é um alvo do CDP no WebView2 (o teste usa os contextos de
execução, e escolhe o do `reader.html`, porque as seções `blob:` têm a mesma origem); o Ctrl+A do
teste mandava o caractere junto e escrevia um "a" na busca; e um clique logo depois de o teste rolar a
grade não abria o livro. A lista virtualizada do React Aria desliga os eventos de ponteiro do seu
conteúdo enquanto rola, até 300 ms depois do último evento de rolagem (`ScrollView`, para rolar sem
custo), e o teste clicava uns 30 ms depois de rolar: o clique caía na lista, não no cartão. Eram as duas
falhas das passadas 9 e 12 (os dois cartões que pedem rolagem); o teste agora espera a lista parar.

## 7. Limitações conhecidas

- **Paginado, a página inteira.** Depois de um relayout, a página mostrada é a que contém a âncora, que pode
  estar no meio dela (D3).
- **Paginado, nos cortes.** Cada parte de um documento cortado é uma seção do foliate-js, e o paginado
  começa página nova numa seção nova: num capítulo de mais de 200 KiB, a última página antes do corte pode
  ficar incompleta. O contínuo não tem isso.
- **Emendas com colunas ou floats.** O documento inteiro equilibra as colunas de um contêiner com
  `column-count` em todo o conteúdo, e o texto depois do corte contorna um float que passa dele; em duas
  partes, cada uma faz isso sozinha. Nenhum dos 13 livros reais tem um corte assim.
- **Sumário de DjVu** não é lido; quadrinhos não têm. Num livro fluido, a entrada atual do sumário é a da
  seção (a parte), não a do fragmento dentro dela.
- **Escrita vertical.** A rolagem contínua supõe texto horizontal; um livro em escrita vertical lê-se no
  paginado, que o foliate-js trata.
- **Links `mailto` e `tel`** só se copiam; os da web abrem no navegador pelo botão do diálogo (D12).
- **Capas.** JPEG de 256 px, que fica macio numa tela de alta densidade. São desenhadas uma de cada vez,
  em segundo plano; a de um livro fluido custa normalizá-lo inteiro no worker uma vez.
- **A listagem com 100 mil livros não foi medida.** A consulta escolhe o melhor arquivo de cada documento
  com uma função de janela sobre todos os arquivos, a cada página; o benchmark de 100 mil arquivos da
  Fase 1 mediu a busca, não esta listagem. Um evento do indexador relê a primeira página.
- **Um clique logo depois de rolar a biblioteca se perde.** Até 300 ms depois do último movimento da
  rolagem, a grade e a tabela do React Aria ignoram o ponteiro; o segundo clique abre o livro.
- **A tipografia vale para todos os livros**, não para cada um.
- **Um livro aberto por vez**, e o tamanho e a posição da janela não são lembrados.
- **A busca** é por título, autor e série, como na Fase 1; não há busca no texto dos livros.

## 8. Como reproduzir

```powershell
cd apps\desktop; npm install; npm run build:app; cd ..\..
.\target\release\gen-fixtures.exe --out fixtures
cargo test --workspace
cd apps\desktop; npm test; cd ..\..
node tools\e2e-alpha.mjs --phase fixtures
node tools\e2e-alpha.mjs --phase corpus --corpus D:\Livros
.\target\release\corpus-check.exe --corpus D:\Livros --work D:\Temp\fluido --flow-only
node tools\reader-check.mjs --cache D:\Temp\fluido\cache
```

## 9. Decisões da auditoria (2026-11-10)

O projeto auditou a Parte 2 e o marco Alpha e os aprovou. As duas decisões que estavam em aberto ficaram assim:

1. **D3: no paginado, a página mostrada depois de um relayout é a que contém o caractere da âncora.** As
   quebras de página continuam as da seção; a âncora continua o mesmo caractere.
2. **D12: o clique num link para fora continua interceptado, e o programa nunca abre o navegador sozinho.**
   O diálogo do link tem dois botões explícitos, **Copiar endereço** e **Abrir no navegador**; o segundo
   abre o endereço no navegador padrão, e só com o clique nele.

Depois da aprovação, D12 foi aplicada como descrito na seção 4, com os testes do validador de endereços em
`links.rs` e a conferência do diálogo no teste na janela (`tools/e2e-alpha.mjs`).
