# ADR 0012: decisões da Fase 7, Parte 1, as anotações do próprio PDF

| | |
|---|---|
| **Data** | 2026-11-23 |
| **Situação** | Aceita na auditoria de 2026-11-23, sem emendas (seção 9) |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo, WebView2 157 |
| **Escopo** | Exportar as anotações do Musaeum para uma **cópia** do PDF, como anotações nativas (ISO 32000-1, compatíveis com Acrobat e PDFium), por atualização incremental, sem nunca gravar no arquivo da biblioteca; importar as anotações nativas de um PDF, sem duplicar o que a obra já tem; exportar e importar XFDF (ISO 19444-1) com um leitor estrito; as quatro ações no painel de anotações do leitor; testes e a passada na janela real |

## 1. Resumo

O painel *Anotações* do leitor, num PDF, ganhou uma seção *No arquivo PDF* com quatro ações:

- **Exportar PDF com anotações (cópia)…** grava, onde a pessoa escolher, um PDF novo: os bytes do livro,
  todos, sem mudar nenhum, e depois deles uma atualização incremental com as anotações do Musaeum como
  anotações do próprio PDF: realces (`/Highlight`), sublinhados (`/Underline`), caixas (`/Square`), traços
  (`/Ink`) e notas fixadas (`/Text`), cada uma com a nota em `/Contents`, a cor, a opacidade, as datas, o
  autor e o `/NM`, que é o UUIDv7 da anotação. Antes de a cópia tomar o nome escolhido, o PDFium a lê e tem
  de achar cada anotação no lugar em que foi posta, e cada anotação que o livro já tinha.
- **Importar anotações do PDF…** traz para o livro as anotações de um PDF com as mesmas páginas: o
  próprio livro (as anotações que ele já trazia, feitas no Acrobat, por exemplo) ou uma cópia anotada em
  outro programa. Uma anotação que a obra já tem, pelo `/NM` ou pelo UUID, não é trazida de novo: importar
  o mesmo arquivo duas vezes não duplica nada.
- **Exportar XFDF…** e **Importar XFDF…** fazem o mesmo pelo XML do Acrobat. Numa biblioteca limpa, com o
  mesmo PDF, o XFDF devolve cada anotação como ela era: o mesmo UUID, o mesmo lugar, tipo, cor e nota.

O livro da biblioteca nunca é escrito. Todo PDF e todo XFDF é lido num processo isolado novo,
`musaeum-pdf-annots.exe`, na mesma sandbox do worker de render do PDF; o processo principal só copia
bytes e renomeia.

## 2. Entregas

| Peça | Onde |
|---|---|
| Objetos com a posição de cada valor (para regravar um dicionário com os próprios bytes) | `crates/musaeum-pdf-annot/src/syntax.rs` |
| A estrutura: `startxref`, seções clássicas, em stream e híbridas, object streams, a árvore de páginas com herança | `crates/musaeum-pdf-annot/src/document.rs` |
| O modelo trocado entre host e worker, e o quadro das páginas | `crates/musaeum-pdf-annot/src/model.rs` |
| Strings de texto (PDFDocEncoding, UTF-16BE com BOM) e datas | `crates/musaeum-pdf-annot/src/text.rs` |
| A leitura das anotações pelos objetos | `crates/musaeum-pdf-annot/src/read.rs` |
| A atualização incremental e as aparências | `crates/musaeum-pdf-annot/src/write.rs` |
| XFDF: escrita e leitor estrito | `crates/musaeum-pdf-annot/src/xfdf.rs` |
| A conversa com o worker e o lado do host | `crates/musaeum-pdf-annot/src/{protocol,client}.rs` |
| O worker e a leitura pelo PDFium | `crates/musaeum-render-pdf/src/{annots_main,annots}.rs` |
| A importação idempotente | `musaeum-library/src/annotations.rs` (`import_annotations`), ferramentas de região `point` e `quads` e a largura do traço em `anchoring/region.rs` |
| Os comandos, o mapeamento e a cópia | `apps/desktop/src-tauri/src/pdf_annotations.rs`; os seletores de arquivo em `picker.rs` |
| A interface | `AnnotationUi.tsx` (`PdfActions`), `ReaderView.tsx`, `compositor/annotator.ts` (`paintOf`) |
| Fixtures | `tools/gen-fixtures/src/annot.rs` (`--only annot`: `annot-classic.pdf`, `annot-streams.pdf`, `annot-hybrid.pdf`) |
| Testes | `musaeum-pdf-annot/tests/structure.rs`, `musaeum-render-pdf/tests/annots.rs`, `musaeum-library/tests/annotations.rs`, testes de unidade do host, `compositor/paint.test.ts`, `tools/e2e-pdf-annotations.mjs`, `corpus-check --pdf-annots` |

## 3. Decisões

### D1. O livro é só lido; a cópia é ele e uma atualização

Exportar escreve só no caminho que a pessoa escolheu no diálogo do sistema, e recusa o próprio livro
(pelo caminho e pela identidade do arquivo no volume, que pega um link físico) e qualquer arquivo que a
biblioteca tenha. A cópia é escrita num arquivo temporário ao lado do destino (`.<nome>.musaeum-….tmp`):
primeiro os bytes do livro, lidos pelo mesmo handle somente leitura e passados pelo BLAKE3, que tem de ser o
da revisão aberta (senão o livro mudou no disco e nada é gravado); depois a atualização; `sync_all`.
Então o worker abre a cópia com o PDFium e a confere (D10). Só então o temporário é renomeado para o
destino; qualquer falha o apaga. O worker nunca abre nem escreve arquivo: recebe handles somente leitura e
devolve bytes.

### D2. Quem lê e quem escreve

O PDFium não diz o número de objeto de uma página nem dá o dicionário cru, então não serve para escrever
uma atualização incremental que preserve tudo. A escrita usa um leitor da estrutura próprio, novo, em Rust,
com limites em cada passo e que recusa o que não entende (nada é consertado: uma atualização escrita sobre
uma estrutura adivinhada seria um arquivo danificado). A leitura das anotações tem dois leitores no
worker: o próprio, quando o arquivo não é cifrado e o PDFium põe as páginas no mesmo lugar (`Auto`), e o do
PDFium nos outros casos, inclusive os cifrados (o PDFium decifra; o leitor próprio não). A conferência da
cópia é sempre do PDFium. O worker roda na política `Lockdown`, a mesma do render de PDF, porque carrega o
PDFium.

O PDFium não dá a cor de uma anotação que tem aparência (`FPDFAnnot_GetColor` recusa); o worker tira a
aparência da sua cópia do documento em memória antes de ler a cor, e nunca salva nada.

### D3. A atualização incremental

Para cada página que recebe anotações, a nova versão do objeto da página é **os bytes do dicionário
original** com só a entrada `/Annots` trocada ou acrescentada; se `/Annots` aponta para um array que é só
dela, quem ganha nova versão é esse array (os bytes dele, com as referências novas no fim); se o array é de
mais de uma página, a página ganha um array próprio. Uma página que estava num object stream é reescrita
como objeto comum, com o mesmo número. A seção de referências é do mesmo tipo da mais nova do arquivo
(tabela clássica, também nos híbridos; ou stream, sem compressão), com `/Prev` para ela, `/Root` e
`/Info` copiados e `/ID` com o primeiro identificador do arquivo e um segundo novo. Nada mais muda: o
catálogo, os marcadores, as anotações que havia, os formulários. Uma atualização sobre outra também
funciona. Arquivos cifrados são recusados para gravação (seção 5).

### D4. Onde cada coisa está

O Musaeum guarda os lugares em frações do quadro da página antes da rotação: o CropBox recortado pelo
MediaBox, como o PDFium o calcula (`CPDF_Page::UpdateDimensions`), o mesmo quadro em que o worker de
render mede os caracteres. Assim `u, v → x = esquerda + u·largura, y = topo − v·altura` serve para toda
página, qualquer que seja o `/Rotate` e onde quer que estejam o CropBox e o MediaBox; a rotação muda só
qual canto de um quad é o de cima, e os quads do Musaeum já estão na ordem do texto (topo esquerdo
primeiro). Os `/QuadPoints` são escritos na ordem do Acrobat (sup. esq., sup. dir., inf. esq., inf. dir.),
que o PDFium e o pdf.js leem; na leitura, um quad que nessa ordem se cruza e na ordem anti-horária do texto
da especificação não, é lido nesta. Antes de gravar, o quadro de cada página que recebe anotações pelo
leitor próprio tem de ser o do PDFium (a 0,01 pt), e o número de páginas o mesmo; senão nada é gravado.

### D5. Que anotação vira o quê

| No Musaeum | No PDF |
|---|---|
| Realce, sublinhado ou nota sobre texto (ou sobre linhas de um escaneado) | `/Highlight` ou `/Underline` com os quads; uma nota é `/Highlight` com a nota |
| Caixa | `/Square` com `/IC` |
| Traço | `/Ink` com `/InkList` e `/BS /W` |
| Nota fixada (nova, vinda de `/Text`) | `/Text` com `/Name /Comment` |

O tipo vai também em `/Subj` (*Realce*, *Sublinhado*, *Nota*) e volta por ele; sem ele, pelo subtipo.
Uma anotação que cobre várias páginas vira uma anotação do PDF por página: a primeira com o UUID como
`/NM`, as outras `UUID#2`, `UUID#3`; a nota vai na primeira. Na importação: `/Highlight` → realce;
`/Underline` e `/Squiggly` → sublinhado; `/Square` → caixa; `/Ink` → um traço por stroke, com a largura;
`/Text` → nota fixada; `/FreeText` → caixa com a nota. `/StrikeOut`, `/Circle`, carimbos, anexos e os
outros são contados e deixados de fora; `/Popup`, `/Link` e `/Widget` não são anotações de leitura e não
são contados; respostas (`/IRT`) e anotações ocultas são contadas e deixadas.

Um realce importado vira anotação **do texto** (com a faixa de caracteres, os quads e a citação, como uma
feita no leitor) quando os quads dele são, a 35 % da altura da linha, os que a camada de texto faz dos
caracteres sob eles; senão vira uma região com os quads dados (ferramenta nova `quads`), com a citação do
texto sob ela, ou do OCR depois, num escaneado.

### D6. A aparência é a do leitor

Toda anotação escrita leva `/AP`: o realce e a caixa preenchidos com a cor em *multiply* (`/BM /Multiply`),
como o compositor os desenha (mínimo com o papel); o sublinhado como uma faixa de 2 % a 12 % da altura
abaixo do texto, na cor de tinta da paleta, como o compositor; o traço com a largura e pontas redondas; a
nota fixada como um quadrado da cor com três linhas, com `NoZoom` e `NoRotate`, como o Acrobat faz as
notas (o PDFium as desenha em pé, no tamanho natural, ancoradas no canto superior esquerdo). `/C` é a cor
desenhada (a de tinta, num sublinhado) e `/CA` a opacidade (1 para as cores da paleta).

### D7. Textos, datas e autor

`/Contents`, `/T`, `/Subj` e `/NM` são PDFDocEncoding quando todo caractere tem código nela (os acentos
do português têm, e também aspas curvas, travessão, reticências) e UTF-16BE com o BOM `FE FF` quando não
(uma seta, um ✓, um ideograma). A nota em Markdown vai como texto. As datas são em UTC
(`D:AAAAMMDDHHmmSSZ`); a leitura aceita qualquer fuso e campos faltando. O autor (`/T`) é o nome do
usuário do Windows (`USERNAME`).

### D8. Identidade e idempotência

O `/NM` de uma anotação do Musaeum é o UUIDv7 dela; o de uma que veio de outro programa é o `/NM` com que
veio. Importar procura, **no documento de destino**, uma anotação cujo `pdf_nm` ou UUID seja esse nome,
apagadas também: achando, não traz. Uma anotação sem `/NM` ganha um nome tirado do que ela é
(`musaeum:` e 32 dígitos do BLAKE3 do tipo, da página, da geometria arredondada e da nota), o mesmo a cada
leitura, e é comparada também, marca a marca, com as que o documento já tem. Uma anotação importada
guarda o UUID com que veio quando nenhuma da biblioteca o tem: numa biblioteca limpa, o XFDF ou a cópia
devolvem cada anotação como ela mesma. Num outro documento da mesma biblioteca (a cópia, aberta como
livro), ela ganha um UUID próprio e guarda o de origem em `pdf_nm`. Uma nova exportação não regrava o que
veio do próprio PDF (ele já está nos bytes).

### D9. As cores

Uma cor da paleta vai como o RGB exato dela e volta pelo nome quando a cor lida está a um passo de 255
dele (o XFDF só tem 8 bits). Qualquer outra vira `#rrggbb`, e `#rrggbbaa` com opacidade; o compositor
desenha essas cores como elas são, sobre o papel branco na proporção da opacidade, e o painel mostra a
amostra.

### D10. A conferência da cópia

Depois de escrita, a cópia é lida pelo PDFium, página a página onde há `/Annots`. Cada anotação escrita
tem de estar lá uma vez, pelo `/NM`, na página e com o subtipo dela, com os quads, o traço e a caixa a
0,001 do quadro; cada anotação que o livro tinha tem de estar lá também; o número de páginas é o mesmo.
Senão a cópia é apagada e nada fica no destino.

### D11. XFDF

A escrita segue o XFDF do Acrobat: um elemento por anotação com `page`, `rect`, `color` e
`interior-color` (`#RRGGBB`), `opacity`, `width`, `flags`, `name`, `title`, `subject`, `date`,
`creationdate`, `icon`, `coords` (os `QuadPoints`), `<contents>` e `<inklist><gesture>`; `<f href>` com o
nome do PDF e `<ids original modified>` com o `/ID` dele. Os lugares vão em espaço do usuário, convertidos
pelo quadro de cada página que o PDFium dá.

O leitor é estrito e roda no worker: um `<!DOCTYPE>` é recusado, então nenhuma entidade pode ser definida
e nada fora do arquivo é lido (XXE) nem expandido (o "bilhão de risos"); uma instrução de processamento é
recusada; uma referência que não seja uma das cinco entidades do XML ou um caractere é recusada; o
arquivo é UTF-8, de até 64 MiB, até 100 mil anotações e 32 níveis. Elementos desconhecidos são ignorados
com o que têm dentro; anotações de tipos que o Musaeum não guarda são contadas. Se o `<ids>` do XFDF não
é o do PDF aberto, a importação acontece e avisa.

### D12. Onde a pessoa escolhe

Os seletores são os do sistema (`IFileSaveDialog`, que pergunta antes de escrever por cima, e
`IFileOpenDialog`), com o filtro do tipo, abrindo na pasta do livro; o nome proposto é
"<livro> (anotado).pdf". O caminho vem sempre do diálogo, no Rust: a página só recebe o relatório. Sob
`--e2e` um teste dá o caminho.

## 4. Resultados

### Gates

| Gate | Resultado |
|---|---|
| `cargo fmt --all -- --check` | limpo |
| `cargo clippy --workspace --all-targets -- -D warnings` | limpo |
| `cargo test --workspace` | 795 testes, 0 falhas (759 na Fase 6) |
| Vitest + `tsc --noEmit` | 212 testes, 0 falhas (209 na Fase 6), e o opcional dos escaneados reais da Fase 5 pulado; `tsc` limpo |
| `tools/check-licenses.mjs` | aceito; nenhuma dependência nova de terceiros (o crate novo usa `blake3`, `flate2`, `quick-xml`, `serde` já aprovados) |

### Testes de integração, com o worker real na sandbox e o PDFium (`musaeum-render-pdf/tests/annots.rs`)

Nos três layouts das fixtures (tabela clássica com uma página de geração 1; object streams e xref em
stream com preditor PNG e uma atualização anterior; híbrido com `/XRefStm`), em cada uma das seis
páginas (0°; `/Rotate 90`; `/Rotate 180`; `/Rotate 270` herdado de um nó da árvore com o texto desenhado
girado; CropBox dentro do MediaBox; MediaBox fora da origem com CropBox), um realce, um sublinhado, uma
caixa, um traço e uma nota fixada, feitos sobre os caracteres que o PDFium mede:

| O que | Resultado |
|---|---|
| O original depois da exportação | igual byte a byte; a cópia começa com todos os bytes dele |
| Lidas de volta pelo PDFium e pelo leitor próprio | 30 de 30 anotações por layout (90 nos três), cada uma pelo `/NM`, na página, com o subtipo, a nota, a cor (a 1/255), a opacidade e as datas; quads, caixas e traços a 2·10⁻⁵ do quadro |
| Desenhadas pelo PDFium, conferidas nos pixels da página como mostrada | cada marca na cor dela sobre o texto (≥ 40 % das amostras; a nota, ≥ 50 % dos pixels do ícone), e nenhuma amostra assim no original |
| O que o livro tinha | o realce com popup, o link e a nota antiga continuam, nas páginas deles |
| XFDF | ida e volta com as 30 anotações; um XFDF com `<!DOCTYPE>` e entidade externa é recusado pelo worker, que continua o mesmo processo e responde ao pedido seguinte |

### No acervo real, só lido (`corpus-check --pdf-annots`)

| Medida | Resultado |
|---|---|
| PDFs | 164, todos lidos pelo worker menos 1 (o *Atkins* truncado, que o PDFium também não abre) |
| Com anotações próprias | 7 livros: 45 realces, 2 caixas, 1 nota fixada, 1 FreeText, 3 círculos e 1 tachado (os dois últimos tipos não são trazidos); fora isso 199.260 links, 61 popups, 48 widgets, 18 carimbos, 2 marcas de inserção |
| Os dois leitores | concordam nos 7 (tipo, página, `/NM` e nota de cada anotação) |
| Cópias com anotações acrescentadas | 18, escritas na pasta de trabalho e lidas pelo PDFium: todas as anotações novas no lugar, todas as antigas lá, o prefixo idêntico ao livro; a maior, de 508 MB, com a atualização feita em 120 ms (mediana 75 ms) |
| Não aceitam gravação | 2: o único cifrado de verdade (*Expert Card Technique*; uma busca textual pela palavra `/Encrypt` achara 9 arquivos, mas só um trailer a tem) e o *CRC Handbook* (2016), que está **truncado** como o Atkins: termina no meio do stream de metadados XMP, sem `startxref` nem `%%EOF`; o PDFium o abre reconstruindo a tabela, e as anotações dele são lidas, mas nada é gravado sobre uma estrutura assim |
| O acervo | igual antes e depois (191 arquivos, 84 pastas, 8,88 GB: tamanho, datas e atributos de cada um); o Windows atualizou a data de último acesso de 248, que é metadado do sistema |

As durações do acervo foram medidas com um jogo aberto na máquina (VALORANT): servem de ordem de grandeza,
não de gate (leitura de um PDF: mediana 72 ms, p95 387 ms, máximo 6,6 s).

### Na janela real (`tools/e2e-pdf-annotations.mjs`, build de release)

Build de release da `fase-7`, uma biblioteca nova numa pasta temporária, a máquina livre (sem entrada havia
3 min, sem jogo e sem vídeo à frente): **23 de 23 passos em 48 s**. A primeira passada parou no arrasto sobre o
texto girado (seção 7, item 7); a segunda passou em 21 de 23 (itens 8 e 9); a terceira, com as duas
correções, passou em todos.

| Passo | O que se confere |
|---|---|
| Anotar | sete anotações feitas na janela: um realce, um sublinhado, uma nota com acentos e caracteres fora do PDFDocEncoding (*Ação, coração e pão — “aspas” → ✓*), um traço, uma caixa, um realce na página com `/Rotate 90` e uma caixa na página com `/Rotate 270` herdado e o texto desenhado girado |
| Exportar a cópia | o painel diz onde a gravou; o livro igual byte a byte, e a cópia é os bytes dele e a atualização; na pasta, só a cópia |
| O PDFium nos objetos da cópia | cada anotação pelo `/NM`, com o subtipo, a página, a nota (a de UTF-16BE inteira) e a cor; o realce com popup, o link e a nota antiga do livro continuam |
| O Musaeum na cópia | aberta como livro, sem anotações do Musaeum, ela mostra nos pixels o que o PDFium desenha, cada marca na cor dela, nas duas páginas giradas também |
| Importar | a cópia nela mesma traz 9 (as 7 e as 2 do livro), e de novo nenhuma; no livro, só as 2 que ele não tinha, e de novo nenhuma; as 7 ficam intactas |
| Exportar de novo | uma segunda cópia não grava outra vez as 2 que vieram do próprio livro |
| XFDF | escrito com as 9, o `<f href>` e o `<ids>`; numa biblioteca limpa, com o mesmo PDF, as 9 voltam com o UUID (ou o `/NM` de origem), o tipo, a cor, a nota e o lugar, e são desenhadas onde estavam |
| XFDF hostil | um `<!DOCTYPE>` com uma entidade externa que aponta para um arquivo desta passada é recusado; nada entra, e o painel diz por quê |
| Livros reais, copiados | MacKay (*Information Theory, Inference, and Learning Algorithms*) e Mankiw (*Introdução à Economia*): as 5 anotações de cada um entram uma vez; uma cópia exportada depois não as grava de novo; o livro do acervo fica como estava |

Com a mesma build, como `CHARS_VERSION` 2 faz as anotações de layout fixo serem ancoradas de novo, as
passadas das anotações da Fase 4 (fixtures: PDF, EPUB e DjVu; fechar e abrir, renomear, mover, segunda
edição) e do OCR da Fase 5 rodaram de novo e passaram sem falha.

Depois das passadas: nenhum processo do Musaeum nem WebView2 dele rodando; nada dos testes em `%TEMP%`;
nada dos testes na Lixeira; o acervo com os mesmos 191 arquivos, 84 pastas e 8.877.205.812 bytes, e a escrita
mais recente em qualquer arquivo dele é de 21/11, antes desta fase.

## 5. Limitações conhecidas

- **PDFs cifrados não recebem anotações.** Mesmo os cifrados só com senha de dono (abertos sem senha,
  como a maioria dos livros "protegidos"): as strings e os streams novos teriam de ser cifrados com a chave
  do arquivo (RC4 ou AES, Algoritmos 1 a 2.B da norma), o que esta parte não implementa. A exportação diz
  isso e não grava nada. **A importação deles funciona**, pelo PDFium, que os decifra. No acervo é 1 de
  164 (seção 4).
- **Estruturas que o leitor próprio não lê não recebem anotações**: uma tabela de referências quebrada, que
  o PDFium conserta ao abrir, um filtro que não seja Flate num object stream. Nada é consertado (D2).
- **A largura do traço de um `/Ink` cifrado**: o PDFium só lê `/Border`, não `/BS`; num arquivo cifrado,
  um traço vem com 1 pt quando só tem `/BS`. Nos outros, o leitor próprio lê `/BS`.
- **Tipos não trazidos:** `/StrikeOut` (tachado), `/Circle`, `/Polygon`, `/Line`, carimbos, anexos, sons.
  São contados e o relatório diz quantos ficaram de fora. `/Squiggly` vem como sublinhado e `/FreeText`
  como caixa com a nota (o texto não é desenhado na página); exportados de novo, vão como `/Underline` e
  `/Square`.
- **Respostas (`/IRT`) e anotações ocultas** ficam de fora; o Musaeum não tem conversa em anotações.
- **Não há `/Popup`.** O Acrobat mostra a nota de um realce sem ele, no painel de comentários e ao passar o
  mouse; o PDFium não mostra notas.
- **O que não tem lugar numa página** (órfãs, e as notas do livro todo, da Fase 1) não vai para o PDF; o
  relatório diz quantas.
- **Uma anotação que cobre páginas** é, no PDF, uma por página (`UUID`, `UUID#2`…); outro programa as vê
  como várias.
- **Assinaturas digitais e certificação:** a atualização incremental mantém intactos os bytes assinados,
  mas um documento certificado contra mudanças (`/Perms /DocMDP`) acusará a mudança num leitor que
  verifica assinaturas. Não testado com um arquivo assinado.
- **Texto que corre na vertical no espaço do próprio PDF não se seleciona por arrasto** (limitação da
  Fase 4, achada aqui): a ferramenta *Texto* agrupa os caracteres em linhas horizontais do quadro
  guardado, e numa página de texto desenhado girado (o caso de um escaneado de lado endireitado por
  `/Rotate`) as linhas são verticais nele. Realces vindos de um PDF sobre esse texto entram ancorados nele
  e são exportados certos (seção 4), e as ferramentas *Caixa* e *Traço* funcionam; selecionar
  arrastando, não. Fica para uma parte seguinte.
- **Linearização:** a cópia de um PDF linearizado continua válida, mas deixa de ser "rápida na web" (o
  leitor vê a atualização e lê o arquivo inteiro), como acontece com qualquer atualização incremental.

## 6. Como reproduzir

```powershell
cargo test -p musaeum-pdf-annot
cargo test -p musaeum-render-pdf --test annots
cargo test -p musaeum-library --test annotations
cd apps\desktop; npm test; npm run build:app; cd ..\..
node tools\e2e-pdf-annotations.mjs
target\release\corpus-check.exe --pdf-annots --corpus D:\Livros --work D:\trabalho --out D:\trabalho\report.json --sample 12
```

## 7. Defeitos achados no caminho

1. **Uma exportação depois de importar as anotações do próprio livro as gravaria de novo** (achado ao
   escrever a passada na janela): elas já estão nos bytes do original, e a cópia teria cada uma duas
   vezes. A exportação agora lê o livro antes e deixa de fora o que veio dele, pelo `/NM` ou pelo nome
   tirado do conteúdo; o relatório diz quantas. Uma anotação de outro programa volta com o `/NM` dela.
2. **O PDFium não dá a cor de uma anotação com aparência** (`FPDFAnnot_GetColor` recusa): o worker tira a
   aparência da sua cópia em memória antes de lê-la (D2).
3. **O PDFium ordena as linhas de uma página `/Rotate 90` como ela é mostrada** (da última para a
   primeira, no texto dessa fixture): o teste procurava a palavra pelo número da linha. Ele procura agora
   em toda a página.
4. **Uma nota fixada não está onde o teste a procurava** (expectativa do teste): com `NoZoom` e
   `NoRotate`, o PDFium a desenha em pé, no tamanho natural, ancorada no canto superior esquerdo da caixa,
   como o Acrobat; o teste olha lá, em escala 1.
5. **Dois testes carregando o PDFium no mesmo processo ao mesmo tempo derrubam o processo**
   (`0x80000003`): o PDFium admite uma thread por processo; os testes que o usam no processo passam um de
   cada vez.
6. **O acervo tem um segundo PDF truncado**, o *CRC Handbook* (seção 4). Nada foi tocado nele.
7. **Os quads de caracteres desenhados girados estavam virados 180°** (defeito da Fase 4, achado na
   primeira passada na janela, quando o arrasto na página de texto girado não selecionou nada). O
   `FPDFText_GetCharAngle` do PDFium mede o ângulo no sentido horário (`atan2(c, a)` da matriz do
   caractere), e o worker o passava a `char_quad`, que espera o anti-horário: num texto que corre página
   abaixo, o topo de cada quad ficava embaixo. Um realce cobria o mesmo lugar, mas um sublinhado sairia
   do lado de cima do texto, e os `QuadPoints` diriam ao Acrobat o contrário. O worker agora troca o
   sinal; `CHARS_VERSION` passou a 2 (Rust e TypeScript), então as anotações guardadas em versão 1 são
   ancoradas de novo ao abrir, e os quads delas refeitos. Um teste novo com o PDFium real
   (`characters_drawn_turned_have_their_top_where_the_text_has_it`) falha sem a correção.
8. **Uma caixa ou um traço de uma página mostrada girada voltava com os cantos em outra ordem** (achado na
   segunda passada na janela, no XFDF levado a uma biblioteca limpa): o host refazia o quad de uma
   `/Square` (ou `/Text`) a partir do canto superior esquerdo do quadro guardado, e as faixas de um `/Ink`
   no quadro guardado, enquanto o leitor os faz na página como mostrada e então os guarda. O lugar era o
   mesmo, mas a região não era a mesma número a número, e o canto onde a caixa começa mudava com a
   rotação. O host agora os faz como o leitor (do canto superior esquerdo como mostrado; as faixas na
   página como mostrada, com a largura e a altura dela); um teste de unidade confere nas quatro rotações
   que a região volta canto por canto.
9. **A passada na janela procurava no XFDF o UUID de uma anotação de outro programa** (expectativa do
   teste): ela sai com o `/NM` com que entrou (D8), como no PDF. O teste agora procura esse nome.

## 8. Pontos para a auditoria

1. **PDFs cifrados:** importar sim, exportar não nesta parte (seção 5). A alternativa é implementar o
   manipulador de segurança padrão (RC4 de 40/128 bits, AES-128 e AES-256, R2 a R6) com senha de usuário
   vazia, com dependências de criptografia novas (MD5, RC4, AES, SHA-2).
2. **O leitor da estrutura é próprio** (D2), e não uma biblioteca de terceiros: nenhuma dependência nova
   entrou para isso, e ele recusa o que não entende em vez de consertar.
3. **A ordem dos `QuadPoints`** é a do Acrobat na escrita, e a leitura aceita as duas (D4).
4. **O tipo vai em `/Subj`** com os nomes em português (*Realce*, *Sublinhado*, *Nota*), que o Acrobat
   mostra como o assunto do comentário (D5).
5. **O autor (`/T`) é o nome do usuário do Windows** (D7); a alternativa é um campo nas preferências.
6. **A idempotência é por documento** (D8): a mesma anotação pode existir no livro e na cópia, quando a
   cópia é aberta como livro, e é importada para ela; no livro, não duplica.
7. **Nada de `/Popup`** (seção 5).
8. **As notas fixadas e as caixas com texto livre** entram como regiões novas (`point`) e caixas com nota
   (D5).
9. **Cores fora da paleta** viram `#rrggbb[aa]` e são desenhadas como são (D9).
10. **A correção dos quads girados toca a Fase 4** (seção 7, item 7): `CHARS_VERSION` 2 faz toda anotação
    de layout fixo ser ancorada de novo uma vez, ao abrir o livro.
11. **Selecionar texto vertical no quadro do PDF** fica para depois (seção 5).
12. **O acervo tem um segundo PDF truncado**, *2016 - CRC handbook of chemistry and physics* (29 MB):
    termina no meio do stream de metadados XMP, sem tabela de referências, `startxref` nem `%%EOF`, como o
    Atkins. O PDFium o abre reconstruindo a tabela e o Musaeum o lê, mas não grava anotações nele. Vale
    trocá-lo por uma cópia íntegra, como o Atkins. Nada foi tocado nele.

## 9. A auditoria (2026-11-23)

Os doze pontos da seção 8 foram aceitos como estão implementados:

1. PDFs cifrados: importar sim, exportar não, sem dependências de criptografia novas.
2. O leitor da estrutura é próprio e estrito.
3. `QuadPoints` na ordem do Acrobat na escrita; a leitura aceita as duas.
4. `/Subj` em português por ora; passa pela tradução da interface na Fase 8.
5. O autor (`/T`) é o nome do usuário do Windows por ora; o campo nas preferências fica para a Fase 8.
6. A idempotência é por documento.
7. Nada de `/Popup`.
8. `/Text` entra como região `point` e `/FreeText` como caixa com nota.
9. Cores fora da paleta em `#rrggbb[aa]`.
10. A ancoragem de novo, uma vez, por `CHARS_VERSION` 2.
11. Selecionar por arrasto texto desenhado na vertical fica para um ciclo posterior.
12. O CRC Handbook truncado fica como está no acervo; a pessoa o troca fora do app.
