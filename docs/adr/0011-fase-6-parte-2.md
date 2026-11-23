# ADR 0011: decisões da Fase 6, Parte 2, o marco Beta

| | |
|---|---|
| **Data** | 2026-11-23 |
| **Situação** | Aceita na auditoria de 2026-11-23, sem emendas (seção 9) |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo, WebView2 157 |
| **Escopo** | Seleção de vários livros (Ctrl+clique, Shift+clique, Ctrl+A) e edição em lote atômica com proveniência; revisão das sugestões de reconciliação e das obras que podem ser uma só, e a fusão de obras sem perder nada; busca no texto dos livros (índice Tantivy em segundo plano, com orçamento), com trechos, página ou seção, e o leitor aberto no ponto achado; importação de uma biblioteca do Calibre (`metadata.db`, ou os `metadata.opf`), só lida; o teste de janela da Beta e a validação só de leitura no acervo real |

## 1. Resumo

A biblioteca da Parte 1 se organizava um livro de cada vez. Agora:

- **Vários livros de uma vez.** Na grade e na lista, Ctrl+clique acrescenta ou tira um livro da escolha,
  Shift+clique escolhe os do meio, e Ctrl+A escolhe todos os da lista, inclusive os que a lista ainda
  não carregou. Com dois ou mais livros escolhidos, o painel *Detalhes* passa a editar todos juntos:
  autores, série, ano, idioma, avaliação e estado de leitura para todos; categorias, etiquetas e
  coleções acrescentadas a todos ou tiradas de todos, com a conta de em quantos cada uma já está. Um
  campo em que os livros diferem mostra "(vários)" até a pessoa escrever nele. *Salvar em N livros* grava
  tudo numa só transação, só os campos tocados, e cada um passa a ser da pessoa.
- **Revisar.** Uma entrada nova no lado, com o número do que espera: os arquivos que podem ser o mesmo
  (um que voltou mudado, um que tomou o lugar de outro com outros identificadores, as sugestões de
  reconciliação da Fase 1) e as obras que podem ser um só livro (o mesmo ISBN ou outro
  identificador, o mesmo título do mesmo autor). Cada par se aceita ou se recusa; recusado, não volta. A
  fusão escolhe a obra que fica e traz para ela os documentos das outras inteiros, com as anotações, a
  posição de leitura e o histórico de cada um, e as categorias, etiquetas, coleções e campos; o que a que
  fica não tem é tirado das outras. A fusão também se pede do painel de vários livros.
- **No texto.** Com algo digitado na busca, *Títulos / No texto* escolhe onde buscar. No texto, cada
  livro achado aparece com as páginas (PDF, DjVu) ou seções (EPUB e os outros fluidos) em que estão
  todas as palavras pedidas, um trecho em volta com o que casou realçado, e um clique abre o livro ali:
  na página, na altura da palavra; num livro fluido, no lugar exato. O texto vem da camada de texto dos
  livros fixos, das linhas já lidas por OCR nos escaneados (Fase 5) e do texto normalizado dos fluidos
  (Fase 3), e é lido num índice em segundo plano, devagar, sem atrapalhar o leitor. Voltando do livro, a
  biblioteca está como foi deixada: a mesma busca, o mesmo modo, o mesmo filtro.
- **Do Calibre.** *Importar do Calibre…* escolhe a pasta de uma biblioteca do Calibre, mostra o que ela
  tem (livros, arquivos, colunas personalizadas e o tipo de campo de cada uma) e a traz: cada livro com
  os seus formatos numa obra, título, autores, série e número, etiquetas, avaliação, identificadores,
  comentários (a sinopse) e as colunas personalizadas como campos personalizados. A pasta do Calibre é
  só lida, nem o `metadata.db` muda; ela passa a ser acompanhada como as outras, e um arquivo com os
  mesmos bytes de um que a biblioteca já tem é o mesmo livro (BLAKE3), não um segundo.

## 2. Entregas

| Peça | Onde |
|---|---|
| Esquema: proveniência `calibre`, `work.description`, `work_pair_dismissed`, `calibre_book` | `migrations/library/0008_beta.sql` |
| Estado do índice de texto por conteúdo | `migrations/cache/0006_fulltext.sql` |
| Edição em lote e o resumo do que vários livros têm em comum | `musaeum-library/src/batch.rs` |
| Obras que podem ser uma, a revisão, a fusão | `musaeum-library/src/merge.rs` (e `merge_empty_work` em `content.rs`) |
| Índice e busca de texto (Tantivy) | `musaeum-library/src/fulltext.rs` |
| Leitura de uma biblioteca do Calibre e a importação | `musaeum-library/src/calibre.rs` |
| O leitor do texto dos livros em segundo plano, a página e o lugar de um achado | `apps/desktop/src-tauri/src/fulltext.rs` |
| Comandos: lote, revisão, Calibre, seleção de todos | `apps/desktop/src-tauri/src/{taxonomy,review,calibre,shelf}.rs` |
| Seleção múltipla, painel de vários livros, revisão e fusão, resultados no texto, diálogo do Calibre | `apps/desktop/src/app/library/{LibraryView,BatchPanel,Review,TextResults,CalibreDialog}.tsx` |
| O leitor aberto num achado | `apps/desktop/src/app/reader/ReaderView.tsx` (`target`) |
| Testes: núcleo, interface, janela | `musaeum-library/tests/beta.rs`, `taxonomy-model.test.ts`, `tools/e2e-beta.mjs` |
| Uma biblioteca do Calibre sintética, do mesmo esquema nos testes Rust e na janela | `tools/lib/calibre-schema.sql`, `calibre-schema-after.sql`, `calibre-library.mjs` |

## 3. Decisões

### D1. A escolha guarda livros, não posições na lista

A grade e a lista usam a seleção múltipla do React Aria em modo `replace` (um clique escolhe só aquele,
Ctrl+clique alterna, Shift+clique estende), com `disallowEmptySelection` como na Parte 1. A escolha é um
mapa de documento para obra, guardado por si: a lista é virtual e carrega de 400 em 400, e Ctrl+A tem de
escolher todos os livros da lista, e não só os carregados. Por isso
Ctrl+A pergunta à biblioteca (`library_select_all`, com a mesma busca e o mesmo filtro da lista) e recebe
todos os pares documento–obra; um clique dado antes da resposta vence. Uma obra escolhida (um livro, ou
os vários formatos de um mesmo livro) se edita no painel de um livro; duas ou mais, no de vários. Indo
para outro filtro, ficam escolhidos só os livros que ele lista, como na Parte 1; Esc guarda a escolha.

### D2. Um lote é uma transação, e só o que se tocou

`edit_works(obras, BatchEdit)` confere tudo antes de escrever e aplica a cada obra, numa só transação,
a mesma edição que o painel de um livro faz (`apply_edit`, da Parte 1): ou todas mudam, ou nenhuma. O
`BatchEdit` só tem o que faz sentido para vários livros (pessoas, série, ano, idioma, avaliação, estado;
categorias, etiquetas e coleções a acrescentar e a tirar); o título, os identificadores e o número na
série são de cada livro e ficam de fora. Cada campo tocado ganha proveniência `user` em cada obra, e só
ele: um teste e a janela conferem que o título e as pessoas, não tocados, continuam do arquivo.
Acrescentar uma categoria, etiqueta ou coleção que uma obra já tem não a duplica; uma etiqueta nova nasce
pelo nome, uma vez. O limite é 20 mil obras por lote, para que uma transação não segure a biblioteca por
muito tempo.

### D3. A proveniência tem uma ordem

`field_provenance.origin` aceita agora `calibre`, entre a leitura do arquivo e a pessoa: `user` (3) >
`calibre` (2) > `embedded` e `filename` (1). Uma leitura de arquivo não escreve num campo `user` nem num
`calibre`; uma importação do Calibre não escreve num `user`; e a fusão, quando leva um campo de uma obra
para a outra, leva a origem junto e fica com a de ordem maior. A migração refaz a tabela com o novo
`CHECK`, preservando as linhas.

### D4. O que pode ser um, e o que a pessoa disse que não é

Duas obras podem ser uma quando têm um identificador em comum (ISBN, ISSN, DOI, OCLC, LCCN, ASIN, URI;
`other` não conta, porque é qualquer coisa) ou o mesmo título do mesmo primeiro autor, comparados
dobrados (`fold`, da Parte 1); um título sem autor diz pouco e não conta. Um grupo grande de títulos
iguais mostra no máximo 12 de cada vez, e a revisão no máximo 500 pares. Nada se funde sozinho: a
biblioteca só sugere. "São livros diferentes" guarda o par em `work_pair_dismissed` (o menor id
primeiro), e ele não volta; se uma das obras some numa fusão, as recusas dela vão junto. As sugestões de
reconciliação de documentos (Fase 1: o mesmo id interno, o mesmo File ID, o mesmo caminho, o mesmo
começo de arquivo) se aceitam e recusam pelos comandos que já existiam, agora com uma tela.

### D5. A fusão leva os documentos inteiros

`merge_works(fica, outras)` move os documentos das outras obras para a que fica. As anotações, a
posição de leitura e o histórico são do documento, não da obra, então vão junto sem cópia nem
reescrita: as âncoras continuam exatas (a janela confere uma nota feita antes da fusão). Os campos da
obra que fica vencem; os que ela não tem (série, número, ano, idioma, avaliação, sinopse, e as pessoas
se ela não tem nenhuma) vêm da outra, com a origem. Categorias, etiquetas, coleções (no lugar em que a
outra estava) e valores de campos personalizados se juntam sem duplicar, e o vínculo com o Calibre
(`calibre_book`) passa para a que fica. Tudo numa transação.

### D6. O índice de texto é um cache, em Tantivy

Tantivy 0.27.0, fixado (`=0.27.0`), só com `mmap` e `lz4-compression`; licença MIT, conferida pelo
`cargo-deny` com as outras. O índice fica na pasta de cache (`cache/fulltext/v1`), não no `library.db`:
se se perde, lê-se de novo; o estado de cada conteúdo (feito, vazio, falhou; quantas unidades e
caracteres; quantas linhas de OCR havia) fica no `cache.db`, por hash BLAKE3, então dois arquivos com os
mesmos bytes são lidos uma vez, e um arquivo renomeado ou movido não é lido de novo. A versão do
esquema (`FULLTEXT_VERSION`) faz tudo ser relido quando muda. A unidade é a página de um livro fixo ou o
documento lógico de um fluido (um capítulo, em geral): é o que a busca mostra e o que se abre. O texto é
dobrado como os nomes (minúsculas e sem acentos: `AsciiFoldingFilter`) com posições, para frases. A
busca aceita palavras e "frases entre aspas", todas obrigatórias, e nada mais: nenhuma linguagem de
consulta é interpretada, então `AND`, `título:` ou `*` digitados são só texto. O trecho mostrado e a
posição do achado saem dos offsets dos tokens, em unidades UTF-16, as da página.

### D7. Lido devagar, e mais devagar enquanto se lê

O leitor do texto é uma thread do app em modo de fundo do Windows (prioridade de E/S e de CPU baixas,
como o indexador da Fase 1), que descansa o dobro do tempo que trabalhou a cada 250 ms de trabalho, e
nove vezes enquanto um livro está aberto no leitor. Ela acorda com os eventos da biblioteca (um livro
novo, uma nova edição, linhas novas de OCR) e, ao começar, tira do índice o que não está mais na
biblioteca. O texto de um livro fixo vem do pool de render da Fase 2 (um worker, a camada de texto
página a página); uma página sem texto usa as linhas de OCR que a Fase 5 já leu, e nenhum OCR novo é
feito para isso; um quadrinho não tem texto. O de um fluido vem do normalizador da Fase 3, no processo
isolado de sempre. O progresso aparece acima dos resultados ("Lendo o texto dos livros: 40 de 162").

### D8. Abrir no achado

Um achado numa página abre o livro na página e na altura da palavra: o app acha, na camada de texto da
página, a palavra que contém a posição do achado e manda o leitor fixo ao seu canto (`go-to-point`, o
mesmo de pular para uma anotação). Num livro fluido, a posição no documento lógico vira um intervalo em
CFI do leitor pelo motor de âncoras da Fase 3 (`reader_range`), e o leitor vai ao começo dele. O leitor
vai lá uma vez, e a pessoa lê dali.

### D9. O Calibre, só lido

O `metadata.db` é aberto como arquivo imutável (`file:…?mode=ro&immutable=1`, `SQLITE_OPEN_READ_ONLY`):
assim o SQLite não pega trava nem escreve diário, e nada na pasta muda. Enquanto o Calibre escreve nele
(um `metadata.db-journal` ou `-wal` ao lado), a importação recusa, porque uma leitura imutável de um banco
em escrita pode ver um estado partido. O banco é entrada como um livro: `trusted_schema` desligado, o modo
defensivo ligado, nenhum valor maior que 1 MiB, e só tabelas são lidas, nunca as views e os triggers do
Calibre (que chamam funções que só o Calibre tem; o teste põe uma assim e confere que nada a executa).
Sem `metadata.db`, os `metadata.opf` de cada pasta servem. Um caminho de livro do banco que sai da pasta
(`..`, um disco, uma barra invertida) é ignorado.

Um livro do Calibre só é aplicado quando todos os seus arquivos no disco já foram lidos pela biblioteca:
então se sabe o hash de cada um, e um arquivo com os bytes de um já conhecido é aquele documento. Os
formatos de um livro do Calibre viram uma obra; `calibre_book` guarda qual obra veio de qual livro de
qual biblioteca, então uma segunda importação não duplica nada e acrescenta o que faltava. O que vem do
Calibre tem origem `calibre` (D3); um valor de campo personalizado nunca escreve por cima de outro, e as
etiquetas se somam. As colunas personalizadas viram campos: texto, comentários e série → texto;
enumeração → lista de opções (com as opções); número e avaliação → número; data → data; sim/não →
sim/não; as calculadas (`composite`) não vêm, porque são o Calibre que as calcula. Uma coluna cujo nome
já é de um campo de outro tipo vira "Nome (Calibre)". A avaliação do Calibre (0 a 10, meias estrelas)
vira estrelas arredondando para cima; o idioma ISO 639-2 (`por`) vira a etiqueta de duas letras (`pt`);
um identificador desconhecido vira `other` com o esquema junto (`goodreads:12345`); os comentários em
HTML viram texto, com os parágrafos.

### D10. A biblioteca é lembrada enquanto um livro está aberto

O leitor substitui a biblioteca na janela, e antes a busca e o filtro se perdiam ao abrir um livro. Com a
busca no texto, abrir um achado e voltar para o próximo é o uso normal, então o app guarda a busca, o
modo (títulos ou texto), o filtro do lado e a vista (capas ou lista) e os devolve ao voltar. A escolha
de livros não volta: ela é de antes de abrir.

## 4. Resultados

### Gates

| Gate | Resultado |
|---|---|
| `cargo fmt --all -- --check` | limpo |
| `cargo clippy --workspace --all-targets -- -D warnings` | limpo (três tipos compostos ganharam nome: `ScalarRow`, `ColumnField`, `Titled`) |
| `cargo test --workspace` | 759 testes, 0 falhas (742 na Parte 1); `tests/beta.rs` tem 6, e há testes de unidade novos em `calibre.rs`, `fulltext.rs` e no `fulltext.rs` do app |
| Vitest + `tsc --noEmit` | 209 testes, 0 falhas (201 na Parte 1); `tsc` limpo |
| `tools/check-licenses.mjs` | `cargo-deny` aceita as dependências Rust, Tantivy e as suas incluídas; npm: 17 entregues e 96 só de desenvolvimento; 24 componentes vendorizados |

### Na janela real (`tools/e2e-beta.mjs --phase synthetic`, build de release)

24 de 24 passos, em 18 s. A primeira passada teve 21 de 23: o Shift+clique estendia a partir do livro
clicado (defeito, seção 7), e o teste contava só os pares de obras no número do *Revisar*, que também
conta, com razão, uma sugestão da Fase 1 (seção 7). O que a passada confere:

| Passo | Como |
|---|---|
| Ctrl+clique, Shift+clique, Ctrl+A, Esc | cliques e teclas pelo protocolo do WebView2, com os modificadores; a escolha lida do app |
| Dois livros editados juntos | série, ano, 4 estrelas, "Lido" e uma etiqueta nova; nos dois, cada campo tocado é `user`, e o título e as pessoas continuam do arquivo |
| Índice do texto | os seis livros lidos em segundo plano |
| Achado numa página | "QUASAR" acha a p. 4 de um PDF de 5 páginas, com o trecho realçado; o clique abre o PDF com a p. 4 na tela e o localizador nela |
| De volta à biblioteca | a mesma busca, no texto |
| Achado num livro fluido | "ornitorrinco" acha a seção 2 de um EPUB; o clique o abre no documento do CFI do achado; ali se faz uma nota |
| Revisar | o par com o mesmo ISBN, o par com o mesmo título e autor e a sugestão de "mesmo identificador interno"; recusados, a sugestão e o segundo par somem e não voltam |
| Fusão | as duas obras com o mesmo ISBN viram uma, com os dois documentos, a etiqueta e a série; a nota feita antes continua lá e exata quando o livro é reaberto |
| Calibre | o diálogo mostra 3 livros e 7 colunas, e que a calculada não é trazida; importa 2 (o terceiro não tem arquivo); *O Cortiço* vem com os dois formatos numa obra, série e número, etiquetas, 4 estrelas, ISBN, sinopse, idioma, ano e as seis colunas, cada campo `calibre`; *Dom Casmurro*, com os mesmos bytes de um livro da estante, é aquele livro, com a série e a etiqueta do Calibre; nenhum arquivo da pasta do Calibre muda (tamanho, datas e SHA-256 de cada um) e nenhum diário fica ao lado do `metadata.db` |
| Fechado e aberto de novo | a fusão, o par recusado e os livros do Calibre continuam; o texto do livro do Calibre está no índice |

As passadas das fases anteriores rodaram de novo com esta build, porque a escolha de livros mudou
debaixo delas: `e2e-taxonomy` 15 de 15 (27 s); `e2e-alpha --phase fixtures` e `e2e-annotations --phase
fixtures` sem falha (5,7 e 1,6 min).

### No acervo real, só lido (`--phase real`)

O acervo tem hoje 191 arquivos de livros (275 entradas com as pastas, 8,88 GB); eram 157 na Fase 2.
Foram duas passadas, a segunda esperando a leitura do texto terminar:

| Medida | Resultado |
|---|---|
| Listar | 191 livros em 10,4 e 7,0 s, cada arquivo um livro |
| Busca no título | uma palavra do título de um livro o acha |
| Ler o texto de todos, em segundo plano, com o orçamento de D7 | 191 de 191 em 19,2 min; 176 com texto, 85.522 páginas ou seções, 214,8 milhões de caracteres (cerca de 186 mil por segundo de relógio, trabalhando um terço do tempo) |
| Sem texto | 14 PDFs só de imagem (escaneados sem OCR): o `pdftotext` do poppler, outra implementação, também acha 0 caracteres neles, e os produtores são de digitalização (`tiff2pdf`, ABBYY, Acrobat Scan, conversores do Google Books e do issuu) |
| Falhou | 1: *2018 - Atkins Physical Chemistry*, que o PDFium não abre; o arquivo está truncado (termina no meio dos metadados XMP, sem `startxref` nem `%%EOF`), e o poppler também o recusa |
| Busca no texto ("the") | 49 livros em 32 e 30 ms |
| Abrir um achado | o PDF abre com a página do achado na tela |
| O acervo | igual antes e depois, nas duas passadas e na interrompida (275 entradas: tamanho, datas de criação e de modificação, atributos) |

Uma terceira passada, antes dessas duas, foi interrompida quando alguém usou o computador, dez minutos
depois de começar a ler o texto; o app fechou e o acervo foi conferido igual. Não houve trava: a janela
só esperava o índice. A ferramenta agora mostra a busca no texto durante a espera, com o progresso, e o
escreve no relatório a cada minuto.

### O que não foi medido

O tamanho do índice em disco para o acervo inteiro (a pasta temporária foi apagada ao fim); a edição em
lote no limite de 20 mil obras; o tempo de uma busca no índice de uma biblioteca de 100 mil livros.

## 5. Limitações conhecidas

- **Escaneados sem OCR não são achados no texto.** Só entram as páginas cujas linhas a Fase 5 já leu (as
  marcadas); os 14 do acervo ficam de fora até isso. Fazer o OCR do livro inteiro em segundo plano seria
  outra decisão, de custo alto (dezenas de ms por linha).
- **Sem radicais nem idiomas.** "livro" não acha "livros": a dobra só tira maiúsculas e acentos. Línguas
  sem espaço entre palavras (japonês, chinês) viram palavras longas demais e ficam de fora, porque tokens
  de mais de 60 caracteres são descartados.
- **Até 100 livros e 5 trechos por livro** numa busca no texto. Com um filtro do lado, a busca pede os
  200 melhores e o filtro escolhe entre eles; um livro do filtro além desses não aparece.
- **Uma fusão não se desfaz**, e o diálogo diz isso. As obras que sumiram não voltam; os documentos, com
  o que está neles, ficam inteiros na que fica.
- **O lote não edita título, identificadores, número na série, sinopse nem campos personalizados:** são
  de cada livro.
- **Do Calibre não vêm** as capas (a biblioteca desenha as suas, dos arquivos), os formatos que não estão
  no disco, nem os idiomas além do primeiro. Uma coluna personalizada de série vem como texto, com o
  número entre colchetes ("Nome [3]"); uma coluna de texto com vários valores vem com eles separados por
  vírgula. Uma biblioteca do Calibre no meio de uma
  escrita é recusada até ele terminar.
- **O índice de texto é um cache:** se for apagado, a biblioteca o lê de novo, em segundo plano (19 min
  para o acervo deste computador).

## 6. Como reproduzir

```powershell
cargo test -p musaeum-library --test beta
cd apps\desktop; npm test; cd ..\..
cd apps\desktop; npm run build:app; cd ..\..
node tools\e2e-beta.mjs --phase synthetic
node tools\e2e-beta.mjs --phase real --corpus D:\Livros
```

## 7. Defeitos achados no caminho

1. **Shift+clique estendia a partir do livro clicado** (achado na janela). A escolha controlada era um
   `Set` refeito a cada render, e o React Aria guarda no objeto `Selection` que entrega o ponto de onde a
   escolha se estende (`anchorKey`); um `Set` novo o perde. O app agora devolve ao React Aria o mesmo
   objeto que ele deu, enquanto esse objeto tem os mesmos livros.
2. **O número do *Revisar* parecia errado e estava certo** (expectativa do teste). O segundo EPUB do
   teste tem o identificador interno do primeiro e outros bytes, então a biblioteca pergunta, como desde
   a Fase 1, se ele é o mesmo arquivo mudado, além de sugerir as duas obras como uma. O teste agora
   confere a sugestão e a recusa pela tela.
3. **A biblioteca esquecia a busca ao abrir um livro** (achado ao desenhar a busca no texto): o leitor
   substitui a biblioteca, e tudo dela se perdia. Ver D10.
4. **A espera pelo índice parecia uma trava** para quem olhava a janela durante a passada no acervo: a
   ferramenta ficava minutos sem mudar a tela. Agora mostra a busca no texto, cujo progresso anda.
5. **Um caminho de livro do `metadata.db` poderia sair da pasta do Calibre** (`..`, um disco, uma barra
   invertida): ele é ignorado, com teste.
6. Menores: três tipos compostos sem nome (clippy), formatação, a expectativa de um teste de armazenamento
   presa à versão anterior do cache (5, agora 6), e a data indefinida do Calibre (`0101-01-01`), que não
   deve dar ano.

## 8. Pontos para a auditoria

1. **A versão.** Os números de versão continuam `0.0.1` (Cargo, `package.json`, `tauri.conf.json`), como
   na tag `v0.1.0-alpha`. Proposta: pôr a tag `v0.2.0-beta` no commit desta parte e deixar as versões
   como estão; a alternativa é subir as três para `0.2.0-beta` no mesmo commit.
2. **A busca no texto:** todas as palavras são obrigatórias, as frases vão entre aspas, sem radicais e
   sem linguagem de consulta (D6).
3. **O orçamento do índice:** ele descansa o dobro do que trabalha, e nove vezes com um livro aberto; o
   acervo inteiro leva 19 min (D7).
4. **Escaneados:** só as linhas já lidas por OCR entram no índice; nenhum OCR é feito para a busca
   (seção 5).
5. **Quando duas obras podem ser uma:** um identificador em comum, ou o mesmo título do mesmo primeiro
   autor; nada se funde sozinho, e o par recusado não volta (D4).
6. **A fusão:** a obra que fica vence, o que ela não tem vem das outras, e a fusão não se desfaz (D5).
7. **O Calibre:** é lido como arquivo imutável, recusado durante uma escrita e acompanhado como as outras
   pastas; o que vem dele (`calibre`) vence o arquivo e perde para a pessoa; um valor de campo nunca
   escreve por cima de outro, e as etiquetas se somam (D3, D9).
8. **O lote:** só pessoas, série, ano, idioma, avaliação, estado e as listas, até 20 mil obras (D2).
9. **O acervo tem 191 arquivos, não 162**, e um deles, *2018 - Atkins Physical Chemistry*, está truncado
   (seção 4): vale trocá-lo por uma cópia íntegra. Nada foi tocado nele.

## 9. A auditoria (2026-11-23)

Os nove pontos da seção 8 foram aceitos como estão implementados:

1. As versões continuam `0.0.1` (Cargo, `package.json`, `tauri.conf.json`); a Beta é a tag anotada
   `v0.2.0-beta` na `main`, no commit desta parte.
2. Busca no texto com todas as palavras obrigatórias e frases exatas entre aspas, sem radicais nem
   segmentação de línguas sem espaço (CJK) por ora.
3. O orçamento do índice, 2× de descanso e 9× com um livro aberto, em segundo plano.
4. Escaneados entram na busca só pelas linhas já lidas por OCR.
5. Duas obras podem ser uma por um identificador comum ou pelo mesmo título e primeiro autor, sem
   fusão automática.
6. A fusão não se desfaz, com o aviso no diálogo, e a obra que fica vence.
7. O Calibre: proveniência `calibre` entre o arquivo e `user`, etiquetas somadas, "Nome (Calibre)" num
   conflito de tipo, capas desenhadas pela biblioteca a partir dos arquivos.
8. O lote: até 20 mil obras, sem os campos que são de cada livro.
9. O arquivo truncado do Atkins fica como está no acervo; a pessoa o troca fora do app.
