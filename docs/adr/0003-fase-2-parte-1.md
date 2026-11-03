# ADR 0003: decisões da Fase 2, Parte 1

| | |
|---|---|
| **Data** | 2026-11-03 |
| **Situação** | Construída e medida no Windows, aguardando a auditoria do projeto |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo |
| **Escopo** | O pipeline de layout fixo para PDF. CBZ, CBR e DjVu ficam para as próximas partes da Fase 2 |

## 1. Resumo

O pipeline de layout fixo agora rasteriza na escala exata da tela, lê a geometria e o que o catálogo pede
sem rasterizar nada, monta páginas duplas com a paridade impressa, tem os dois modos de leitura com a máquina
de estados do plano e guarda a posição na biblioteca. Tudo foi validado nos 157 livros reais de
`C:\Marte\Archives\DigitalLibrary\Books`, sem que nenhum arquivo do acervo mudasse.

Os resultados que mais pesam:

1. **Nítido em um quadro depois do zoom.** Em 6 fatores de zoom que nenhuma potência de 2 alcança, em dois PDFs
   do acervo, a tela ficou nítida 13,1 a 13,5 ms depois de a animação parar: um quadro a 75 Hz, contra a meta de
   100 ms. Os tiles da escala final são pedidos durante a animação (D3).
2. **Worker morto, leitura segue.** Workers terminados de fora, no meio de render pesado, foram substituídos em
   P50 de 20 a 26 ms e máximo de 31 ms. O tile seguinte chegou em no máximo 65 ms, contra a meta de 1 s.
3. **Acervo inteiro sem falha do Core.** 144 PDFs e 83.001 páginas medidos, 36 páginas inteiras renderizadas em
   12 livros escolhidos por serem difíceis, 0 recusas do PDFium, 0 falhas do Core, 0 tiles perdidos.
4. **Somente leitura comprovado.** Tamanho, datas de criação e modificação e atributos das 228 entradas do acervo
   ficaram idênticos. O Windows atualizou a data de último acesso de todas, porque essa atualização está ligada
   na máquina (seção 6).
5. **O cancelamento no meio do render tem um limite que vem do PDFium.** Numa página vetorial ele interrompe o
   desenho. Num escaneamento pesado, só depois da decodificação da imagem: 17,6 ms em P50 (D4).
6. **Uma corrida da Fase 1 apareceu e foi corrigida.** O watcher devolvia o controle antes de estar armado, e uma
   mudança nesse intervalo se perdia (D10).

## 2. Entregas

| Entrega | Como foi testado | Resultado |
|---|---|---|
| Git consolidado | `fase-1` commitada (1db4ca5), `main` avançado sem merge, `fase-0-parte-2` e `fase-1` apagadas, `fase-2` criada | Árvore limpa no início da fase |
| Continuidade de caminho refinada | 8 cenários reais em `continuity.rs`: tipo diferente, `/ID[0]` diferente, `dc:identifier` diferente, mesmo ID, ID só de um lado, provisório até o hash, bytes de outro documento, nota feita no meio | Só vira revisão com o mesmo tipo e sem conflito de ID. No conflito: documento próprio, sugestão com a evidência, o arquivo antigo ausente e nada perdido |
| Primeira migração real | Biblioteca no esquema da Fase 1, aberta pela versão nova | Cópia `before-upgrade-from-v1` feita antes, dados preservados, uma segunda abertura não copia de novo |
| Escala exata | Worker real renderiza 1,37 px/pt e as bordas caem no pixel previsto. No app, 6 zooms em 2 PDFs reais | Nítido 13,1–13,5 ms após parar. 0 quadros em branco |
| Cancelamento no meio do render | Página vetorial sintética e escaneamento real de 2,2 MB por página | Vetorial: interrompido no meio. Escaneado: 10 de 10 cancelados, latência P50 17,6 ms e máximo 22,7 ms |
| LRU com orçamento | Testes do cache e o benchmark de zoom | Orçamento em bytes (`--tile-budget-mb`), padrão de 4 telas. 52 a 83 despejos por sessão, nenhum de tile em uso |
| Supervisor | 8 mortes por `TerminateProcess` em PDF sintético e 6 em escaneamento real, sempre com render pesado em andamento | Substituto pronto em P50 20 e 26 ms, máximo 31 ms. Sessão segue |
| `page_geometry` no `cache.db` | 144 PDFs reais medidos, gravados e lidos de volta | 144 de 144 idênticos na volta. Medir: P50 33 ms, máximo 396 ms em 1.493 páginas |
| `/PageLayout` e `/Direction` | 11 testes, incluindo cada truncamento e cada byte trocado de um arquivo. No acervo, pelo worker isolado | 36 `SinglePage`, 3 `OneColumn`, 2 `TwoPageRight`, 103 sem. 12 com `L2R`, 0 com `R2L` |
| Layout misto | O código do compositor aplicado às 83.001 páginas reais | 0 páginas cortadas no contínuo, 0 linhas que não cabem no paginado, 0 erros de paridade |
| Modos, mola e persistência | 15 testes do controlador com relógio próprio, mais testes da mola, da entrada e do `reading_state` | Ver D6 a D9 |
| Acervo real | `tools/corpus-check`, abaixo da prioridade normal, em 24,6 s | Ver seção 5 |

**Testes.** 429 em Rust no workspace (eram 389) e 125 em TypeScript, em 19 arquivos. Todos passam, com `clippy -D
warnings`, `fmt` e `tsc --noEmit` limpos, licenças conferidas, e uma rodada completa não deixa nada no `%TEMP%` nem
na Lixeira.

## 3. Como foi medido

- **O acervo real** foi lido pelo `tools/corpus-check`, que lista a árvore inteira antes de tudo e de novo no fim,
  pela listagem de diretórios da biblioteca padrão, sem abrir arquivo, e falha se algo diferir. A biblioteca, o
  cache e o relatório ficaram numa pasta temporária fora do acervo. Os workers rodam no mesmo sandbox do app.
- **O zoom** foi medido no app de verdade (`--scenario zoom`), com janela de 1600 × 900, no caminho C, em dois livros
  do acervo: um texto (*Discrete Mathematics with Applications*) e o escaneamento mais pesado (*Color and Light*).
  O tempo até nítido conta do último quadro em movimento ao primeiro quadro sem nenhum tile substituto.
- **O layout** foi validado aplicando o próprio código do compositor (`spreads.ts`, `view.ts`) às páginas medidas
  do acervo, guardadas no `cache.db`.

Os limites:

- **O app não foi exercitado por uma pessoa.** O modo paginado, a barra de leitura e a gravação da posição pelo app
  estão cobertos por testes das partes. O benchmark de zoom roda no modo contínuo, que é o que os cenários roteirizam.
- **Os números de zoom são da compilação anterior a dois ajustes finais:** uma refatoração das vistas, sem mudança de
  comportamento, e a correção da velocidade de rolagem (D8). Não medi de novo porque a máquina estava em uso.
- **Trackpad e roda do mouse** foram distinguidos com sequências de eventos sintéticas, modeladas no que o Chromium
  produz no Windows. Nenhum dispositivo físico foi usado.
- **Uma máquina, 75 Hz, uma GPU dedicada.** 120 Hz, GPU integrada e macOS continuam sem medição.

## 4. Decisões

### D1. Continuidade de caminho: provisória até os bytes falarem

**Decisão.** A regra do ADR 0002, D2, continua decidindo pelo diretório, mas o vínculo é provisório. A tabela
`replaced_file` guarda o que a instância era. Quando o hash chega, o tipo do conteúdo novo é comparado com o do
anterior. Quando a extração chega, `/ID[0]` e `dc:identifier` são comparados com os do documento, antes de qualquer
coisa ser gravada nele. Conflito é só quando os dois lados têm o identificador e ele difere.

Num conflito, o resultado é o que a varredura daria sem o atalho: a instância volta a ser o arquivo antigo, ausente
no último lugar visto, e o arquivo novo ganha instância, documento e obra próprios, com uma sugestão `same_path`
que diz o motivo. Notas feitas sobre os bytes novos e a posição de leitura tomada neles vão junto com eles.

**Tipo do conteúdo.** Vem dos primeiros bytes, que o hash completo já lê: PDF, ZIP, DjVu, RAR, 7z, MOBI, XML ou
desconhecido. Comparar assinaturas não é interpretar o arquivo, então roda fora do sandbox. Revisões da Fase 1 não
têm tipo gravado e valem pelo formato do nome.

**Por quê guardar e desfazer, em vez de esperar para vincular.** O caso comum é o safe save. Esperando, cada safe
save mostraria um arquivo ausente até o hash, e um arquivo só na nuvem, que não pode ser lido, ficaria assim para
sempre.

**Bytes que já são de outro documento.** Na Fase 1, a instância passava para esse documento e o antigo ficava sem
nenhuma instância. Agora o antigo fica com a instância ausente, como deve.

### D2. Escala em unidades de 1/4096 pixel por ponto

**Decisão.** O tile é identificado por página, escala, coluna e linha, e a escala é um inteiro em 1/4096 px/pt, de
1/64 a 32 px/pt. O protocolo dos workers passou à versão 2.

**Por quê esse passo.** Arredondar para 1/4096 px/pt move a borda de uma página comum cerca de um décimo de pixel, e
a da maior página que o PDF admite, menos de dois. O compositor desenha a página na própria escala quantizada, então
cada texel cai num pixel. Uma página grande demais para a grade numa escala recebe a maior escala que cabe, e só
ela, ou uma miniatura muito pequena, é reamostrada.

**Chaves no compositor.** Página, escala, coluna e linha não cabem nos 53 bits exatos de um `double`. O cache usa
dois níveis: página com escala, depois a posição na grade, que tem no máximo 1024 tiles de lado.

### D3. Durante o zoom, o que houver; ao parar, o exato, já pedido

O planejador desenha cada pedaço de página com o que o cache tiver: a escala exata; senão outra escala, até 4
vezes mais fina ou qualquer mais grossa, recortada para cobrir o pedaço. É tudo ou nada por pedaço, para não haver
buraco no meio de um substituto. Os tiles da escala de destino são pedidos assim que o alvo do zoom fica 50 ms sem
mudar, para onde a câmera vai parar, e não para onde está. Por isso chegam antes de a animação acabar.

A animação de zoom dura cerca de 0,3 s. Começou com 0,74 s, por uma constante e um limite de parada conservadores
demais, o que o primeiro benchmark mostrou.

### D4. Cancelamento: o que a API progressiva do PDFium permite

O cancelamento usa a API progressiva que já existia: o worker consulta a célula de cancelamento quando o PDFium
pergunta se deve pausar, e também antes e depois de carregar a página. O PDFium pergunta entre objetos da página.
Numa página vetorial isso acontece o tempo todo, e o render para no meio. Num escaneamento, a página é uma imagem só,
e a decodificação dela não é interrompida: no *Color and Light*, cancelando aos 7,6 ms de um render de 30 ms, a
resposta veio em 17,6 ms. Nenhum resultado cancelado foi entregue.

Ir além exige decodificar imagens fora do PDFium ou mudar o PDFium, o que fica junto do build próprio dele.

### D5. Geometria e catálogo sem rasterizar

O worker 0 lê o tamanho de cada página e manda junto, numa mensagem nova, o que o catálogo pede. O PDFium não expõe
`/PageLayout`, então um leitor em Rust seguro segue a estrutura do arquivo: `startxref`, tabela clássica ou stream de
referências cruzadas com preditor PNG, objeto solto ou dentro de object stream, atualizações incrementais,
arquivos híbridos e `/ViewerPreferences` indiretas. Tudo tem limite, e o que ele não entende vira "sem resposta",
porque um layout errado é pior que o padrão. Ele roda nos dois processos isolados que já leem o arquivo: o `ingest`,
para a biblioteca, e o worker de render, para o leitor.

O resultado vai para o `cache.db`, pelo BLAKE3 dos bytes. Ao reabrir, nenhum worker mede de novo. Se o cache estiver
errado sobre o documento, o pool pede a medição a um worker, e o custo é tempo, nunca o documento.

### D6. Páginas duplas pela paridade impressa

Cada página tem um lado pelo seu número, nunca pelo que veio antes. Paisagem fica sozinha. Uma página de retrato sem
o par certo ganha um espaço vazio do tamanho dela, para que as seguintes continuem frente a frente. `TwoColumnLeft` e
`TwoPageLeft` começam os pares na primeira página. Os outros valores de dupla, e páginas duplas pedidas pelo leitor,
deixam a primeira sozinha, como num livro. Da direita para a esquerda, a ordem visual de cada par se inverte.

`rendition:spread` e `page-spread-*` são do EPUB de layout fixo e ficam para quando ele entrar.

### D7. Os dois modos e a posição

- **Contínuo:** uma escala física para todas as páginas, a confortável, limitada para que a linha mais larga caiba
  na janela. O zoom do leitor multiplica isso e, acima de 1, rola na horizontal.
- **Paginado:** cada linha cabe inteira, com margem, numa escala própria. As linhas ficam lado a lado numa faixa, e
  a passagem é a faixa deslizando. Com proporções diferentes, as duas páginas aparecem cada uma no seu tamanho durante
  o deslize. Esse é o significado que dei a "animar a mudança de proporção". Mudanças de janela ou de páginas duplas
  se aplicam no mesmo quadro.
- **Posição:** um Locator, página mais ponto (u, v) no canto superior esquerdo da viewport. Só muda quando o leitor se
  move. Trocar de modo, de zoom ou de janela reposiciona a câmera a partir dele, e é por isso que ir e voltar entre os
  modos devolve a posição exata.

### D8. Máquina de estados, mola e entrada

- **Estado.** `{ kind: 'paginated', animate }` ou `{ kind: 'continuous' }`. O contínuo não tem o campo, e a
  preferência fica ao lado do modo e volta com o paginado. `prefers-reduced-motion` define o valor inicial.
- **Mola.** Criticamente amortecida, resolvida de forma exata e não integrada: não oscila, não depende do tamanho do
  quadro e dá o mesmo resultado em qualquer divisão do tempo. Retomar o alvo mantém posição e velocidade, e um dedo que
  pega a página no meio a segura onde ela está.
- **Gesto.** O arraste move a faixa exatamente com o dedo, no máximo uma tela. Ao soltar, passa se foi rápido ou longe
  o bastante. Sem animação, o gesto troca a página no mesmo quadro ao passar de um limiar.
- **Roda e trackpad.** A roda recebe suavização própria. O trackpad, que já tem o momentum do Windows, é aplicado como
  vem. A classificação usa o formato dos eventos e mantém a classe durante o gesto.
- **Velocidade.** Medida de quadro a quadro, qualquer que seja o que moveu a coluna. Na primeira versão só contava a
  suavização da roda, e o pedido de só placeholders em rolagem rápida nunca disparava pelos outros caminhos.

### D9. `reading_state` na biblioteca

A tabela guarda página, u, v, progresso, modo, preferência de animação, zoom, páginas duplas e a revisão em que a
posição foi tomada. O app grava 0,4 s depois de a posição parar de mudar, e só para o documento aberto. Ao juntar
dois documentos, fica a posição mais recente. A biblioteca é opcional: com `--library` ou com a biblioteca padrão, se
já existir. Abrir um arquivo nunca cria uma biblioteca, e um arquivo fora dela abre normalmente, sem guardar a posição.

### D10. Watcher armado antes de devolver o controle

`watch_directory` passou a esperar a primeira chamada a `ReadDirectoryChangesW`. O Windows só registra mudanças a
partir dela, e com a máquina carregada depois de um religamento, 1 em cada 3 rodadas dos testes do watcher perdia o
evento. No produto, era um buraco entre armar o watch e varrer. Um teste novo escreve no instante em que o watch volta,
30 vezes: sem a correção falhou em 2 de 3 rodadas, com ela passa.

## 5. Métricas

**O acervo real** (157 arquivos: 144 PDFs e 13 EPUBs, 71 pastas, 7,3 GiB):

| Medida | Resultado |
|---|---|
| Varredura | 357 ms, 157 arquivos, 0 ilegíveis |
| Hash completo e extração | 7,7 s para 7,82 GB, 157 de 157, 0 recusas do worker |
| Nova varredura sem mudanças | 13 ms, 0 bytes lidos |
| `/ID[0]` | 133 de 144 PDFs. Nenhum compartilhado, nenhuma sugestão |
| `dc:identifier` | 13 de 13 EPUBs |
| Abrir o PDF no worker | P50 42 ms, P95 155 ms, máximo 463 ms |
| Medir as páginas | P50 33 ms, P95 152 ms, máximo 396 ms. 2.524 páginas em 39 ms |
| Orientação | 129 só retrato, 2 só paisagem, 13 mistos. O mais misto: 8 retratos e 282 paisagens |
| Amostra renderizada | 12 livros, 36 páginas inteiras, de 0,93 a 4,4 px/pt. 0 tiles perdidos ou falhos, todos opacos e com conteúdo. 7 a 83 ms por página inteira |
| Páginas duplas | 42.228 linhas, 40.773 pares, 1.112 paisagens sozinhas, 343 espaços de paridade |
| Falhas do Core | 0 |

**Zoom no app**, por segmento de 1,5 s, nos dois livros:

| Medida | Resultado |
|---|---|
| Até nítido depois de parar | 13,1 a 13,5 ms, 12 de 12 zooms |
| Quadros em branco | 0 |
| Trabalho por quadro | P99 2,2 ms, máximo 3,5 ms (meta 8,3 ms) |
| Quadros com substituto | 22 a 28 por zoom, a duração da animação |
| Orçamento de GPU | 80 MiB, 52 a 83 despejos |

## 6. Limitações conhecidas

- **Data de último acesso.** Ler um arquivo faz o Windows atualizar essa data, e na máquina a atualização está ligada
  (`DisableLastAccess = 2`). Aconteceu nas 228 entradas do acervo. Não é uma escrita das ferramentas, que só abrem para
  leitura, e evitar exigiria mudar a configuração do volume, como administrador.
- **Classificação texto, escaneado ou híbrido**, do item 2.5 do plano, não foi feita. Só a orientação, como pedido.
  Exige carregar cada página, o que precisa ir para segundo plano.
- **Miniaturas em disco** não existem ainda.
- **Cancelamento em escaneamentos** espera a imagem decodificar (D4).
- **Arquivos fora da biblioteca** não guardam posição.
- **Dispositivos físicos.** Trackpad e roda não foram testados com hardware, e o modo paginado não foi medido no app.

**Itens da seção 6 do ADR 0002 que esta parte fecha:** PDFs reais no leitor de `/ID` (133 de 144) e a migração real
(da versão 1 para a 2, nos dois bancos).

## 7. Como reproduzir

```text
cargo test --workspace
cd apps/desktop && npm test && npx tsc --noEmit
cargo build --release -p corpus-check -p musaeum-render-pdf -p musaeum-library --bins
target\release\corpus-check.exe --corpus <ACERVO> --work <PASTA FORA DO ACERVO> --sample 12
cd apps/desktop && npm run build:app
target\release\musaeum-desktop.exe --pdf <PDF> --bench zoom.json --scenario zoom
```

O `corpus-check` sai com código diferente de zero se qualquer arquivo do acervo mudar ou se houver falha do Core.
