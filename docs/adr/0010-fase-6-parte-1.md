# ADR 0010: decisões da Fase 6, Parte 1, a organização da biblioteca

| | |
|---|---|
| **Data** | 2026-11-17 |
| **Situação** | Aceita na auditoria de 2026-11-17, com a emenda de D6 (seção 9) |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo, WebView2 157 |
| **Escopo** | Árvore de categorias e subcategorias (caminho materializado, chave de ordem fracionária, criar, renomear, mover subárvore sem ciclos, vínculo N:N com obras) ao lado da faceta de pastas do disco; etiquetas; coleções manuais ordenadas; coleções inteligentes por regra; campos personalizados tipados; o painel de detalhes que edita os metadados de uma obra, protegido pela proveniência dos campos contra reimportações |

## 1. Resumo

A biblioteca só se navegava pelas pastas do disco e pela busca. Agora o lado esquerdo tem, além de
*Todos os livros* e das *Pastas*, as *Categorias* (uma árvore de quantos níveis se quiser, até 16), as
*Etiquetas*, as *Coleções* (listas feitas à mão, na ordem que a pessoa dá) e as *Coleções inteligentes*
(uma regra: "autor contém Sagan", "ano entre 1980 e 1999 e avaliação ≥ 4", "na categoria Ciências",
"Situação é emprestado"). Escolher qualquer um mostra os livros dele, com a busca por cima; uma categoria
mostra também os livros das subcategorias, e uma coleção, na ordem dela. Cada item tem um menu ("⋯") com o
que se pode fazer a ele, e cada seção um "+" para criar.

Um clique num livro agora o seleciona e abre o painel *Detalhes* à direita; o duplo clique, ou Enter,
abre o livro. O painel edita a obra inteira: título, pessoas e o papel de cada uma (autor, tradutor,
organizador…) na ordem certa, série e número, ano, idioma, identificadores (ISBN conferido pelo dígito
verificador), avaliação, estado de leitura, categorias, etiquetas (uma nova nasce ao ser digitada),
coleções e os campos personalizados (texto, número, data, sim/não, lista de opções, endereço web). Nada se
grava até *Salvar*; então só o que mudou, e cada campo mudado passa a ser da pessoa: ao lado de cada um, o
painel diz de onde ele veio ("do arquivo", "editado"), e nada lido de um arquivo depois (uma reimportação,
uma nova edição do arquivo, o mesmo livro achado em outro lugar) escreve por cima.

## 2. Entregas

| Peça | Onde |
|---|---|
| Esquema: `category`, `work_category`, `tag`, `work_tag`, `collection`, `collection_item`, `smart_collection`, `work_identifier`, `custom_field`, `work_custom_value` | `migrations/library/0007_taxonomy.sql` |
| Chaves de ordem fracionárias | `musaeum-library/src/order.rs` |
| Categorias, etiquetas, coleções, coleções inteligentes, campos personalizados | `musaeum-library/src/taxonomy.rs` |
| Regras das coleções inteligentes, de JSON a SQL só com parâmetros | `musaeum-library/src/smart.rs` |
| A obra no editor: leitura, edição, proveniência, identificadores | `musaeum-library/src/metadata.rs` |
| Listagem por categoria (com as de baixo), etiqueta, coleção (na ordem), coleção inteligente | `musaeum-library/src/catalog.rs` |
| `fold()` (sem maiúsculas nem acentos) no SQL; datas | `musaeum-library/src/sql.rs` |
| Comandos do app | `apps/desktop/src-tauri/src/taxonomy.rs` |
| Lado da biblioteca, diálogos, painel *Detalhes* | `apps/desktop/src/app/library/{Sidebar,dialogs,Inspector}.tsx`, `taxonomy-model.ts` |
| Testes: núcleo, interface, janela | `musaeum-library/tests/taxonomy.rs`, `taxonomy-model.test.ts`, `tools/e2e-taxonomy.mjs` |

## 3. Decisões

### D1. Categorias: caminho materializado e chave fracionária

Cada categoria guarda o caminho dos ids da raiz até ela, `/3/14/15/`. Uma categoria e tudo o que está
debaixo dela são as linhas cujo caminho começa pelo dela: listar os livros de "Ciências" com os de
"Biologia" e "Física" é uma comparação de prefixo, sem recursão. Mover uma subárvore reescreve esse
prefixo num só comando; a mudança é recusada se o destino começa pelo caminho da própria categoria (ela
mesma ou uma das suas), e se a subárvore passaria de 16 níveis. A ordem entre irmãos é uma chave
fracionária em base 62 (`0-9A-Za-z`, que ordena como texto): entre duas chaves sempre existe outra, então
subir ou descer uma categoria muda uma linha. As mesmas chaves ordenam as coleções, os livros de uma
coleção e os campos personalizados. Acrescentar sempre no fim faz a chave crescer um dígito a cada 61
itens; mil itens acrescentados em ordem cabem em chaves de até 18 caracteres (teste).

### D2. Nomes comparados como uma pessoa compara

"Física" e "  fisica " são o mesmo nome: os nomes são comparados dobrados (minúsculas, sem os acentos dos
alfabetos latinos, um espaço entre palavras), pela função `fold`, que o SQL também tem. Dois irmãos na
árvore, duas etiquetas, dois campos personalizados não podem ter o mesmo nome dobrado; primos podem
("Física" em Ciências e em Literatura). A mesma dobra faz "historia" achar "História" numa regra.

### D3. Etiquetas, coleções e campos são da pessoa, sem proveniência

Nenhum arquivo diz em que categoria, etiqueta ou coleção um livro está, então essas tabelas não têm
proveniência e nada da leitura de arquivos escreve nelas. Excluir uma categoria tira da árvore ela e as de
baixo, e os livros delas continuam na biblioteca e nas outras categorias; excluir uma etiqueta ou uma
coleção, o mesmo. Um campo personalizado de lista não perde uma opção que algum livro tem.

### D4. Coleções inteligentes: uma regra em JSON, nunca SQL

A regra é `{"match": "all"|"any", "rules": [{"field", "op", "value"}]}`, guardada como JSON. Campos e
operadores vêm de uma lista fechada (título, autor, série, idioma, identificador, ano, avaliação, lido %,
estado, formato, adicionado, lido pela última vez, etiqueta, categoria, coleção, pasta, e cada campo
personalizado); cada um aceita só os operadores do seu tipo; os valores vão sempre como parâmetros. A
regra é conferida antes de guardar, e um teste prova que um valor com SQL dentro fica só um texto
procurado. Se a regra nomeia um campo personalizado que foi removido, a coleção diz que não funciona mais
(um "!" no lado) e não acha nenhum livro, em vez de falhar a biblioteca.

### D5. O editor grava só o que mudou, e o que mudou é da pessoa

O painel mantém um rascunho e calcula o que difere da obra lida; *Salvar* manda só isso, e o núcleo
confere tudo antes de escrever (nada é escrito se alguma parte falha). Cada campo da obra que muda ganha
proveniência `user` em `field_provenance`, e a leitura de um arquivo (`apply_extraction`) não toca num
campo `user`. A edição que já existia (Fase 1) dava isso a título, pessoas, série, número na série,
avaliação e estado, mas só podia pôr um valor, nunca tirar (`coalesce`). Esta parte acrescenta ano,
idioma e identificadores, os papéis das pessoas, e limpar: o `WorkEdit` passou a distinguir "não mexer"
de "limpar" (`Option<Option<T>>`), e limpar também torna o campo da pessoa (vazio fica vazio). Dos campos
que um arquivo pode escrever (título, pessoas, idioma, ano, identificadores), um campo `user` não é mais
escrito por nenhum caminho de leitura; os testes cobrem a reimportação, uma nova edição do arquivo e a
fusão com uma obra provisória.

### D6. Identificadores da obra

Os identificadores eram só do documento (o ISBN e o `dc:identifier` do primeiro arquivo). Agora a obra
tem a sua lista, por esquema (ISBN, ISSN, DOI, OCLC, LCCN, ASIN, URI, outro): a migração a preenche com o
que os documentos já diziam (`urn:isbn:`, `doi:` reconhecidos), e cada leitura de arquivo a refaz a partir
dos documentos, enquanto a pessoa não a editar. Um ISBN é guardado sem hífens e só se o dígito
verificador confere.

Emenda da auditoria (seção 9): os identificadores de um documento são os do seu arquivo mais novo. Uma
nova edição com outro ISBN ou outro `dc:identifier` traz os seus, e a lista da obra os segue, a menos que
a pessoa a tenha editado (proveniência `user`, que vale para a lista inteira: editado um identificador,
nenhum muda mais por um arquivo). Um arquivo que não dá nenhum deixa os de antes. O `/ID[0]` de um PDF
continua o do primeiro arquivo: ele só serve para distinguir arquivos, e o PDF mantém essa metade do
`/ID` entre revisões. Antes da emenda, o documento ficava com os identificadores do seu primeiro arquivo
(`coalesce`, desde a Fase 1).

### D7. Quando uma obra se revela outra

Uma obra provisória (antes do hash) pode se revelar a mesma de outra; ela some e os documentos passam
para a outra. As categorias, etiquetas, coleções e valores de campos que a pessoa deu à provisória passam
junto (sem duplicar o que a outra já tinha), e os identificadores seguem a regra de D6.

### D8. Clique seleciona, duplo clique abre

Para haver um livro selecionado para os detalhes, a grade e a lista passaram a selecionar com um clique
e abrir com duplo clique ou Enter (`selectionBehavior="replace"` do React Aria), como num gerenciador de
arquivos. Os testes de janela das fases anteriores já abriam com Enter quando o clique não abria.

Como num gerenciador de arquivos, um clique no livro já escolhido o mantém escolhido
(`disallowEmptySelection`; sem isso, a seleção única do React Aria desmarca o item clicado de novo), e
Esc ou o botão do painel guardam os detalhes. O livro escolhido é guardado por si, não procurado nas
linhas lidas: depois de salvar, a lista é lida de novo a partir da primeira página, e o livro editado
pode estar além dela. Indo para outro filtro ou outra busca em que ele não aparece, ele deixa de estar
escolhido, e não volta sozinho quando se volta; a mesma lista lida de novo o mantém.

## 4. Resultados

Cadeia final de 2026-11-17, com o build release da árvore de trabalho da `fase-6` (sobre `221a121`).

**Testes automáticos**: 742 de Rust (eram 714 no commit da Fase 5; os 28 novos: 10 em
`tests/taxonomy.rs`, um deles o dos 16 níveis, e 18 de unidade, das chaves de ordem, das regras, do editor, da taxonomia, do `fold`
e das datas no SQL) e 201 de TypeScript (eram 189; os 12 novos são do modelo do painel e do lado,
`taxonomy-model.test.ts`), todos passando, com `lines-real.test.ts` de fora como antes.
`cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `tsc` e `check-licenses.mjs`
limpos.

**Na janela real** (`tools/e2e-taxonomy.mjs`, 15 de 15 passos, 43 s): a árvore feita, renomeada e
movida, e o diálogo de mover sem lugar debaixo da própria categoria; uma coleção e um campo de lista
feitos pelo lado; no painel, a lista de categorias mostra só o que tem "Biolo" e fecha ao escolher; o
título, as pessoas e o papel de tradutor, o ano, o idioma (`pt-br` gravado `pt-BR`), a avaliação, o
estado, a categoria, a etiqueta nova, a coleção e o campo gravados, cada campo da obra com proveniência
`user`; um clique no livro escolhido o mantém, Esc guarda os detalhes; a categoria lista os livros da
subcategoria, a etiqueta os seus, a coleção na sua ordem, e um livro sobe nela pelo painel; um livro
deixado para trás pelo filtro não volta escolhido; a coleção inteligente "autor contém SAGAN" acha
*Cosmos*; uma nova edição do arquivo, com outro título e outro autor, não escreve por cima do que a
pessoa pôs; fechado e aberto de novo, tudo continua lá. As passadas de antes, com o mesmo build e a
biblioteca nova, passaram inteiras: `e2e-annotations --phase fixtures` (a Fase 4) e
`e2e-alpha --phase fixtures` (a Alpha; o clique agora escolhe e Enter abre, como o teste já previa).
Nenhuma pasta dos testes ficou em `%TEMP%`, nenhum processo deles ficou rodando, e nada foi para a
Lixeira.

**Escala** (`a_large_tree_lists_and_filters_quickly`, em release, três vezes): 520 categorias em 20
árvores de 6 níveis e 2080 vínculos; a árvore inteira, com as contagens, em 1,3 a 1,5 ms; os livros de
uma categoria com os das de baixo em 0,80 a 0,88 ms.

## 5. Limitações conhecidas

- **Um livro por vez no painel.** Pôr vários livros numa categoria de uma vez existe no núcleo e nos
  comandos (`set_category`, `set_tag`, `add_to_collection` com listas), mas a interface ainda não tem
  seleção múltipla.
- **Mover categorias por menu**, não por arrastar: *Mover para…*, *Subir*, *Descer*.
- **Sem desfazer.** Excluir uma categoria, etiqueta ou coleção pede confirmação e não se desfaz.
- **"Restaurar do arquivo"** não existe: um campo editado continua da pessoa até ela o editar de novo.
- **A busca** não procura por etiqueta nem categoria (as coleções inteligentes fazem isso).

## 6. Como reproduzir

```text
cargo test -p musaeum-library --test taxonomy
cargo test -p musaeum-library --lib
cd apps\desktop && npx vitest run src/app/library && npm run build:app && cd ..\..
node tools\e2e-taxonomy.mjs
```

## 7. Defeitos achados no caminho

- **Chaves que cresciam rápido demais.** Acrescentar no fim punha um dígito novo começando no meio da
  base; a chave crescia um caractere a cada 30 itens. O dígito novo agora começa no segundo valor, e cresce
  a cada 61 (o teste de mil itens em ordem, D1).
- **A fusão de obras falhava** quando a provisória tinha categorias: o mesmo comando SQL copiava e apagava,
  com parâmetros que só a cópia usava (`InvalidParameterCount`). Copiar e apagar viraram dois comandos; o
  teste de D7 cobre o caso.
- **O rascunho do painel se perdia.** O painel refazia o rascunho a partir da obra sempre que a lista de
  etiquetas, categorias ou coleções mudava (criar uma etiqueta no próprio painel bastava): o que se tinha
  digitado e não salvo sumia. A obra lida agora traz os nomes das suas etiquetas, e o rascunho só é refeito
  quando a obra muda.
- **Os identificadores numa nova edição do arquivo.** Um teste esperava que um ISBN novo dentro do arquivo
  trocasse o da obra. Mas o documento guarda os identificadores do seu primeiro arquivo (`coalesce`, desde
  antes desta parte), e os da obra vêm dos documentos. Mudar isso seria mudar uma regra de outra fase; o
  teste foi corrigido para o que o código fazia, e a seção 8 perguntou. A auditoria decidiu pelo arquivo
  novo (seção 9), e o teste agora confere isso, e que uma edição sem identificador deixa o de antes.

Achados pela passada na janela (`tools/e2e-taxonomy.mjs`), que até então nunca tinha passado do passo 1:

- **Um clique no livro escolhido o desmarcava.** A seleção única do React Aria alterna o item clicado:
  clicar de novo no livro cujos detalhes estavam abertos os fechava. Agora o clique o mantém (D8).
- **Uma seleção escondida voltava.** O livro escolhido era procurado nas linhas da lista: num filtro
  que não o lista, os detalhes sumiam, mas a seleção ficava, e eles reapareciam ao voltar a uma lista
  com ele. E, lendo o código (não reproduzido na janela, cujos livros de teste são quatro), numa
  biblioteca de mais de 400 livros salvar um livro além da primeira página fecharia o painel, porque a
  lista é lida de novo desde o começo. O livro escolhido agora é guardado por si (D8).
- **A lista do painel ficava aberta e não filtrava.** Nas categorias, etiquetas e coleções do painel, a
  caixa de acrescentar é um *combo box* do React Aria com a chave escolhida presa em nulo (ela só
  acrescenta). O React Aria fecha a lista quando a chave escolhida muda, o que ali nunca acontece, e a
  abre de novo quando o texto se limpa: depois de escolher uma categoria, a lista de todas as outras
  ficava aberta sobre os campos de baixo, e o clique seguinte do teste, no campo das etiquetas, caiu em
  "Literatura" (uma pessoa faria o mesmo). E, com os itens dados pelo app, o React Aria não os filtra
  pelo que se digita: "Biolo" mostrava todas as categorias. Agora o app abre a lista
  (`menuTrigger="manual"`): num clique na caixa, ao digitar e pelas setas; fecha-a a cada escolha; e
  mostra só o que tem o texto digitado, comparado sem maiúsculas nem acentos (`fold`, como o núcleo,
  D2). Uma primeira versão abria a lista também quando a caixa ganhava o foco, e na janela a lista
  continuava aberta depois da escolha: ao fechar, o popover devolve o foco à caixa. Enter toma a opção
  destacada pelas setas, ou a de nome igual ao digitado, e só então cria uma etiqueta nova. Antes, pela
  leitura do código (não visto na janela), Enter numa etiqueta destacada pelas setas criava, no lugar
  dela, uma etiqueta nova com o texto digitado pela metade.
- **Esc não guardava os detalhes.** Ouvido na prateleira quando a tecla sobe dos itens da grade, Esc não
  chegava lá na janela; ouvido na descida (`onKeyDownCapture`), chega.
- **Dois defeitos do próprio teste.** Enter no botão de uma lista de escolha a abre de novo, como num
  *select*, e o diálogo de mover não era enviado: o teste envia pelo botão. E o botão *Salvar* também
  fica desligado enquanto salva, então o teste lia a obra antes de ela ser gravada: o painel agora diz
  que está ocupado (`aria-busy`) e o teste espera isso acabar.
- **Uma pasta de teste deixada em `%TEMP%`.** Uma passada de `e2e-alpha` passou em tudo e falhou ao
  apagar a sua pasta: o WebView2 ainda fechava o perfil dele (`EPERM`). A pasta foi apagada à mão, e
  `e2e-alpha` e `e2e-reader` agora tentam de novo por cerca de 16 s, como `e2e-annotations` já fazia.

Achado no gate depois da auditoria:

- **Um teste da Fase 4 que falhava por acaso.** `the_schema_refuses_what_would_lose_an_annotation`
  esperava que o esquema recusasse uma anotação inserida com `randomblob(16)` como UUID. Quem a recusa é
  a verificação do UUIDv7, que pede o dígito de versão 7; um blob aleatório o tem uma vez em 16, e então
  a linha é uma anotação válida e o teste falha. O teste agora usa um UUID de versão 4, sempre recusado.

## 8. Pontos para a auditoria

1. **Clique seleciona, duplo clique abre** (D8). Muda o gesto da biblioteca desde a Fase 3: um clique
   abria o livro. Sem isso, não há livro escolhido para o painel. A alternativa seria abrir com um clique
   e mostrar os detalhes por um botão em cada livro.
2. **Excluir uma categoria leva as de baixo** (D3); os livros ficam na biblioteca. A alternativa seria
   subir as subcategorias para o lugar da excluída.
3. **Dezesseis níveis** no máximo na árvore (D1). É um limite contra erro, não de desempenho: a árvore de
   520 categorias e 2080 vínculos lista em 1,3 a 1,5 ms em release (seção 4).
4. **Os identificadores de um documento** são os do seu primeiro arquivo, como antes desta parte (D6,
   seção 7): uma nova edição com outro ISBN não o troca. Manter, ou deixar o arquivo novo trocar o que
   ninguém editou?
5. **Uma coleção inteligente cujo campo foi removido** mostra "!" e não acha nada, em vez de a remoção do
   campo ser recusada (D4). A alternativa seria recusar remover um campo que alguma regra usa.
6. **A faceta de pastas** é a que a biblioteca já tinha (a árvore das pastas do disco sob cada raiz, com
   as de baixo contadas); passou para o lado, ao lado das categorias, e as regras ganharam o campo
   "pasta". O pedido falava em "faceta real de pastas do disco": se era outra coisa, preciso saber o quê.
7. **O nome do painel.** O pedido dizia "Inspetor/Editor de Metadados"; o painel se chama *Detalhes* e
   edita no lugar, sem um modo de leitura separado. O "Inspetor" da Fase 4 é o das anotações.
8. **Seleção de vários livros** (seção 5): o núcleo e os comandos aceitam listas; a interface, um livro
   por vez. Fica para a Parte 2?

## 9. A auditoria (2026-11-17)

Aceitos como estão: o clique que seleciona (1), excluir com as subcategorias (2), os 16 níveis (3), o "!"
da coleção inteligente de um campo removido (5), a faceta de pastas como está, no lado e nas regras (6), e
o nome *Detalhes* (7). A seleção de vários livros (8) entra na Parte 2.

O ponto 4 mudou: um arquivo novo atualiza os identificadores que ninguém editou, e os de proveniência
`user` ficam intocados (D6, emenda). `content.rs`, `apply_extraction`: `isbn` e `dc_identifier` passaram de
`coalesce(antigo, novo)` a `coalesce(novo, antigo)`; `pdf_id0` não mudou. O teste
`what_nobody_edited_follows_the_file` confere o ISBN novo e a edição sem identificador;
`what_a_person_edits_stays_however_the_file_is_read_again` continua conferindo que a lista editada não muda.
