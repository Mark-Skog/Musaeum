//! Every message, in both languages. Grouped by where it is written; a name says what the message
//! is about, not where it is shown.

use crate::Locale;

macro_rules! catalog {
    ($($key:ident { pt: $pt:literal, en: $en:literal $(,)? })*) => {
        /// A message, which [`crate::text`] writes in a language.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum T {
            $($key,)*
        }

        impl T {
            /// Every message, for the tests that hold the two languages together.
            pub const ALL: &'static [T] = &[$(T::$key,)*];

            /// The message as written in `locale`, its values still named.
            pub fn template(self, locale: Locale) -> &'static str {
                match (self, locale) {
                    $(
                        (T::$key, Locale::PtBr) => $pt,
                        (T::$key, Locale::English) => $en,
                    )*
                }
            }
        }
    };
}

catalog! {
    // --- Writing a file where the person chose ---
    InsideLibraryFolder {
        pt: "{path} é uma pasta da biblioteca, e nada é gravado nela: escolha outra pasta",
        en: "{path} is a folder of the library, and nothing is written in it: choose another folder",
    }
    TargetHasNoFolder {
        pt: "o destino não tem pasta",
        en: "the destination has no folder",
    }
    FileNotCreated {
        pt: "o arquivo não pôde ser criado em {folder}: {error}",
        en: "the file could not be created in {folder}: {error}",
    }
    TemporaryNamesInUse {
        pt: "o arquivo não pôde ser criado: nomes temporários em uso",
        en: "the file could not be created: temporary names in use",
    }
    FileNotWritten {
        pt: "o arquivo não pôde ser escrito: {error}",
        en: "the file could not be written: {error}",
    }
    FileNotRenamed {
        pt: "o arquivo não pôde tomar o nome escolhido: {error}",
        en: "the file could not take the name chosen: {error}",
    }

    // --- Annotations in a PDF or an XFDF file: their subject (/Subj) ---
    SubjectHighlight {
        pt: "Realce",
        en: "Highlight",
    }
    SubjectUnderline {
        pt: "Sublinhado",
        en: "Underline",
    }
    SubjectNote {
        pt: "Nota",
        en: "Note",
    }

    // --- Preferences ---
    PreferenceLanguageUnknown {
        pt: "{value} não é um idioma da interface",
        en: "{value} is not a language of the interface",
    }
    PreferenceThemeUnknown {
        pt: "{value} não é um tema",
        en: "{value} is not a theme",
    }
    PreferenceMotionUnknown {
        pt: "{value} não é uma escolha de movimento",
        en: "{value} is not a choice of motion",
    }
    PreferenceAuthorTooLong {
        pt: "o nome do autor tem no máximo {max} caracteres",
        en: "the author's name is {max} characters at most",
    }
    PreferenceAuthorControl {
        pt: "o nome do autor tem um caractere de controle",
        en: "the author's name has a control character",
    }
    PreferenceOcrNone {
        pt: "escolha ao menos um idioma para o OCR",
        en: "choose at least one language for the OCR",
    }
    PreferenceOcrTooMany {
        pt: "o OCR lê no máximo {max} idiomas de uma vez",
        en: "the OCR reads {max} languages at once at most",
    }
    PreferenceOcrUnknown {
        pt: "{code} não é um modelo de OCR verificado nesta instalação",
        en: "{code} is not an OCR model verified in this installation",
    }
    PreferencesNotSaved {
        pt: "as preferências não puderam ser gravadas: {error}",
        en: "the preferences could not be saved: {error}",
    }

    // --- The system's file dialogs: their titles, and the kinds of file they list ---
    FileKindJsonLd { pt: "Anotações W3C (JSON-LD)", en: "W3C annotations (JSON-LD)" }
    FileKindMarkdown { pt: "Markdown", en: "Markdown" }
    FileKindPdf { pt: "PDF", en: "PDF" }
    FileKindXfdf { pt: "XFDF (anotações do Acrobat)", en: "XFDF (Acrobat annotations)" }
    PickExportJsonLd { pt: "Exportar anotações (JSON-LD)", en: "Export annotations (JSON-LD)" }
    PickImportJsonLd { pt: "Importar anotações (JSON-LD)", en: "Import annotations (JSON-LD)" }
    PickExportReportMarkdown { pt: "Exportar relatório (Markdown)", en: "Export report (Markdown)" }
    PickExportReportPdf { pt: "Exportar relatório (PDF)", en: "Export report (PDF)" }
    PickExportPdfCopy { pt: "Exportar PDF com anotações (cópia)", en: "Export PDF with annotations (copy)" }
    PickImportFromPdf { pt: "Importar anotações de um PDF", en: "Import annotations from a PDF" }
    PickExportXfdf { pt: "Exportar anotações em XFDF", en: "Export annotations as XFDF" }
    PickImportXfdf { pt: "Importar anotações de um XFDF", en: "Import annotations from an XFDF" }
    PickCalibreFolder { pt: "Biblioteca do Calibre", en: "Calibre library" }
    PickBooksFolder { pt: "Pasta de livros", en: "Folder of books" }
    PickLibraryFolder { pt: "Pasta da biblioteca (com library.db)", en: "Library folder (with library.db)" }

    // --- The names a save dialog suggests ---
    NameDocumentJsonLd { pt: "{title} — anotações.jsonld", en: "{title} — annotations.jsonld" }
    NameWorksJsonLd { pt: "Anotações de {count} obras.jsonld", en: "Annotations of {count} works.jsonld" }
    NameLibraryJsonLd { pt: "Anotações do Musaeum.jsonld", en: "Musaeum annotations.jsonld" }
    NameWorkReport { pt: "{title} — relatório", en: "{title} — report" }
    NameWorksReport { pt: "Relatório de estudo ({count} obras)", en: "Study report ({count} works)" }
    NamePdfCopy { pt: "{name} (anotado).pdf", en: "{name} (annotated).pdf" }

    // --- JSON-LD and the study reports, from the host ---
    NoAnnotationsToExport { pt: "não há anotações para exportar", en: "there are no annotations to export" }
    FileNotRead { pt: "o arquivo não pôde ser lido: {error}", en: "the file could not be read: {error}" }
    FileTooLarge { pt: "o arquivo é grande demais", en: "the file is too large" }
    NoAnnotationsForReport { pt: "não há anotações para o relatório", en: "there are no annotations for the report" }
    ReportTooManyWorks {
        pt: "um relatório é sobre uma obra ou até duas mil",
        en: "a report is about one work or up to two thousand",
    }
    BookNotReadYet { pt: "a biblioteca ainda não leu este livro", en: "the library has not read this book yet" }
    ReportPdfUnreadable {
        pt: "o PDFium não leu o relatório: {error}: nada foi gravado",
        en: "PDFium did not read the report: {error}: nothing was written",
    }
    ReportPdfMismatch {
        pt: "o PDFium não leu o relatório como foi escrito: nada foi gravado",
        en: "PDFium did not read the report as it was written: nothing was written",
    }

    // --- A PDF's own annotations, and XFDF ---
    CopyIsTheBook {
        pt: "a cópia não pode ser o próprio livro: escolha outro nome",
        en: "the copy cannot be the book itself: choose another name",
    }
    CopyOverLibraryBook {
        pt: "esse arquivo é um livro da biblioteca, e a cópia não é gravada sobre ele: escolha outro nome",
        en: "that file is a book of the library, and the copy is not written over it: choose another name",
    }
    CopyNotCreated {
        pt: "a cópia não pôde ser criada em {folder}: {error}",
        en: "the copy could not be created in {folder}: {error}",
    }
    CopyTemporaryNamesInUse {
        pt: "a cópia não pôde ser criada: nomes temporários em uso",
        en: "the copy could not be created: temporary names in use",
    }
    NativeAnnotationsPdfOnly { pt: "anotações nativas só existem em PDFs", en: "native annotations exist only in PDFs" }
    DocumentNotInLibrary {
        pt: "este documento não está na biblioteca, e só os da biblioteca guardam anotações",
        en: "this document is not in the library, and only the library's keep annotations",
    }
    BookNotInLibrary {
        pt: "este livro não está na biblioteca, e só os da biblioteca guardam anotações",
        en: "this book is not in the library, and only the library's books keep annotations",
    }
    NoAnnotationsForPdf {
        pt: "este livro não tem anotações para levar ao PDF",
        en: "this book has no annotations to take into the PDF",
    }
    BookNotRead { pt: "o livro não pôde ser lido: {error}", en: "the book could not be read: {error}" }
    NoAnnotationOnAPage {
        pt: "nenhuma anotação deste livro tem lugar numa página",
        en: "no annotation of this book has a place on a page",
    }
    AnnotationsAlreadyInPdf {
        pt: "as anotações deste livro já estão todas no PDF",
        en: "this book's annotations are all in the PDF already",
    }
    CopyNotWritten { pt: "a cópia não pôde ser escrita: {error}", en: "the copy could not be written: {error}" }
    BookChangedWhileCopied {
        pt: "o livro mudou no disco enquanto era copiado: nada foi gravado",
        en: "the book changed on the disk while it was copied: nothing was written",
    }
    CopyNotRenamed {
        pt: "a cópia não pôde tomar o nome escolhido: {error}",
        en: "the copy could not take the name chosen: {error}",
    }
    CopyLacksPages {
        pt: "a cópia não tem as páginas do livro: nada foi gravado",
        en: "the copy does not have the book's pages: nothing was written",
    }
    CopyLacksAnnotation {
        pt: "o PDFium não acha a anotação {name} na cópia: nada foi gravado",
        en: "PDFium does not find the annotation {name} in the copy: nothing was written",
    }
    CopyMovedAnnotation {
        pt: "o PDFium lê a anotação {name} em outro lugar da cópia: nada foi gravado",
        en: "PDFium reads the annotation {name} elsewhere in the copy: nothing was written",
    }
    CopyLostAnnotation {
        pt: "uma anotação que o livro já tinha não está na cópia: nada foi gravado",
        en: "an annotation the book already had is not in the copy: nothing was written",
    }
    PdfNotRead { pt: "o PDF não pôde ser lido: {error}", en: "the PDF could not be read: {error}" }
    PdfOtherPages {
        pt: "esse PDF não tem as mesmas páginas deste livro: as anotações cairiam fora do lugar",
        en: "that PDF does not have this book's pages: the annotations would land out of place",
    }
    NoAnnotationsForXfdf {
        pt: "este livro não tem anotações para levar ao XFDF",
        en: "this book has no annotations to take into the XFDF",
    }
    XfdfNotWritten { pt: "o XFDF não pôde ser escrito: {error}", en: "the XFDF could not be written: {error}" }
    XfdfNotRenamed {
        pt: "o XFDF não pôde tomar o nome escolhido: {error}",
        en: "the XFDF could not take the name chosen: {error}",
    }
    XfdfNotRead { pt: "o XFDF não pôde ser lido: {error}", en: "the XFDF could not be read: {error}" }
    XfdfTooLarge { pt: "o XFDF é grande demais", en: "the XFDF is too large" }

    // --- Reading, selecting, searching ---
    SelectionTooManyPages {
        pt: "uma seleção cobre de uma página a algumas dezenas",
        en: "a selection covers from one page to a few dozen",
    }
    SelectionTooManyChapters {
        pt: "uma seleção cobre de um capítulo a algumas dezenas",
        en: "a selection covers from one chapter to a few dozen",
    }
    NothingSelected { pt: "nada foi selecionado", en: "nothing was selected" }
    SearchTooLong { pt: "essa busca é longa demais", en: "that search is too long" }
    BookNotPrepared { pt: "o livro não pôde ser preparado: {reason}", en: "the book could not be prepared: {reason}" }
    BookFileGone {
        pt: "o arquivo deste livro não está mais na pasta",
        en: "this book's file is no longer in its folder",
    }
    MatchNotInBook {
        pt: "o trecho achado não está no livro como ele é agora",
        en: "the match is not in the book as it is now",
    }
    NoOcrModel {
        pt: "não há modelo de OCR verificado: veja tools/fetch-ocr.ps1",
        en: "there is no verified OCR model: see tools/fetch-ocr.ps1",
    }

    // --- The library: what something is called in a refusal ---
    ThingCategory { pt: "uma categoria", en: "a category" }
    ThingTag { pt: "uma etiqueta", en: "a tag" }
    ThingCollection { pt: "uma coleção", en: "a collection" }
    ThingSmartCollection { pt: "uma coleção inteligente", en: "a smart collection" }
    ThingField { pt: "um campo", en: "a field" }
    ThingChoice { pt: "uma opção", en: "a choice" }
    ThingCreator { pt: "uma pessoa da obra", en: "a creator" }
    ThingWork { pt: "uma obra", en: "a work" }
    ThingSeries { pt: "uma série", en: "a series" }
    ThingLanguage { pt: "um idioma", en: "a language" }

    // --- The library: names, categories, tags, collections, fields ---
    NameNeeded { pt: "{what} precisa de um nome", en: "{what} needs a name" }
    NameTooLong {
        pt: "o nome de {what} tem no máximo {max} caracteres",
        en: "the name of {what} is {max} characters at most",
    }
    NameControl {
        pt: "o nome de {what} tem um caractere de controle",
        en: "the name of {what} has a control character",
    }
    NotInList { pt: "{id} não está nessa lista", en: "{id} is not in that list" }
    CategoryNameTaken {
        pt: "já há uma categoria chamada {name} ali",
        en: "there is already a category named {name} there",
    }
    CategoryNameTakenThere {
        pt: "já há uma categoria com esse nome ali",
        en: "there is already a category with that name there",
    }
    TagNameTaken { pt: "já há uma etiqueta chamada {name}", en: "there is already a tag named {name}" }
    FieldNameTaken { pt: "já há um campo chamado {name}", en: "there is already a field named {name}" }
    CategoriesTooDeep {
        pt: "as categorias têm no máximo {max} níveis",
        en: "categories go {max} levels deep at most",
    }
    CategoryUnderItself {
        pt: "uma categoria não pode ficar dentro de si mesma nem de uma das suas",
        en: "a category cannot go under itself or under one of its own",
    }
    TagColorUnknown { pt: "{color} não é uma cor de etiqueta", en: "{color} is not a tag's color" }
    FieldKindUnknown { pt: "{kind} não é um tipo de campo", en: "{kind} is not a kind of field" }
    FieldNoChoices { pt: "um campo do tipo {kind} não tem opções", en: "a field of kind {kind} has no choices" }
    ChoiceCount {
        pt: "um campo de opções tem de 1 a {max} opções",
        en: "a choice field has 1 to {max} choices",
    }
    ChoiceTwice { pt: "a opção {choice} aparece duas vezes", en: "the choice {choice} is there twice" }
    ChoiceInUse {
        pt: "alguma obra tem a opção {choice}, que deixaria de existir",
        en: "some work has the choice {choice}, which would go",
    }
    DescriptionTooLong {
        pt: "uma descrição tem no máximo {max} caracteres",
        en: "a description is {max} characters at most",
    }

    // --- The library: a work's metadata ---
    TextTooLong { pt: "{what} tem no máximo {max} caracteres", en: "{what} is {max} characters at most" }
    TooManyIdentifiers {
        pt: "uma obra tem no máximo {max} identificadores",
        en: "a work has {max} identifiers at most",
    }
    SchemeUnknown {
        pt: "{scheme} não é um esquema de identificadores",
        en: "{scheme} is not a scheme of identifiers",
    }
    IdentifierLength {
        pt: "um identificador tem de 1 a 400 caracteres",
        en: "an identifier is 1 to 400 characters",
    }
    IsbnInvalid { pt: "{value} não é um ISBN válido", en: "{value} is not a valid ISBN" }
    TooManyCreators { pt: "uma obra tem no máximo {max} pessoas", en: "a work has {max} creators at most" }
    RoleUnknown { pt: "{role} não é um papel", en: "{role} is not a role" }
    FieldValueNumber { pt: "o valor de um campo de número é um número", en: "a number field's value is a number" }
    FieldValueBoolean {
        pt: "o valor de um campo de sim/não é verdadeiro ou falso",
        en: "a yes/no field's value is true or false",
    }
    FieldValueText { pt: "o valor de um campo do tipo {kind} é um texto", en: "a {kind} field's value is a text" }
    FieldValueDate {
        pt: "o valor de um campo de data é uma data, AAAA-MM-DD",
        en: "a date field's value is a date, YYYY-MM-DD",
    }
    FieldValueChoice {
        pt: "o valor de um campo de opções é uma das suas opções",
        en: "a choice field's value is one of its choices",
    }
    FieldValueUrl {
        pt: "o valor de um campo de endereço é um endereço http ou https",
        en: "an address field's value is an http or https address",
    }
    FieldValueTooLong {
        pt: "o valor de um campo do tipo {kind} é longo demais",
        en: "a {kind} field's value is too long",
    }
    SeriesPositionRange {
        pt: "a posição numa série vai de 0 a 1000000",
        en: "a position in a series goes from 0 to 1000000",
    }
    YearRange { pt: "um ano vai de -9999 a 9999", en: "a year goes from -9999 to 9999" }
    LanguageTagInvalid {
        pt: "{text} não é um código de idioma (BCP 47)",
        en: "{text} is not a language tag (BCP 47)",
    }
    RatingRange { pt: "uma avaliação vai de 1 a 5", en: "a rating goes from 1 to 5" }
    StatusUnknown { pt: "{status} não é um estado de leitura", en: "{status} is not a reading status" }

    // --- The library: smart collections ---
    SmartRule {
        pt: "uma regra da coleção inteligente ({field} {op}): {why}",
        en: "a smart collection's rule ({field} {op}): {why}",
    }
    SmartTooManyRules {
        pt: "uma coleção inteligente tem no máximo {max} regras",
        en: "a smart collection has {max} rules at most",
    }
    SmartNoField { pt: "esse campo não existe", en: "no such field" }
    SmartNoCustomField { pt: "esse campo personalizado não existe", en: "no such custom field" }
    SmartWrongOperator {
        pt: "essa condição não serve para esse campo",
        en: "this operator does not go with this field",
    }
    SmartValueDate { pt: "o valor é uma data, AAAA-MM-DD", en: "the value is a date, YYYY-MM-DD" }
    SmartValueDays {
        pt: "o valor é um número de dias, até 36500",
        en: "the value is a number of days, up to 36500",
    }
    SmartValueNumber { pt: "o valor é um número", en: "the value is a number" }
    SmartValueText {
        pt: "o valor é um texto de 1 a 400 caracteres",
        en: "the value is a text of 1 to 400 characters",
    }
    SmartValueId { pt: "o valor é um número de registro", en: "the value is an id" }
    SmartValueTwoNumbers { pt: "o valor são dois números", en: "the value is two numbers" }
    SmartValueStatus {
        pt: "o valor é unread, reading, read ou abandoned",
        en: "the value is unread, reading, read or abandoned",
    }

    // --- The library: many works at once, and merging ---
    BatchNoWork { pt: "nenhuma obra foi escolhida", en: "no work was given" }
    BatchTooMany { pt: "no máximo {max} obras de uma vez", en: "{max} works at a time at most" }
    BatchAddAndTake {
        pt: "a mesma categoria ou coleção está sendo posta e tirada",
        en: "the same category or collection is both added and taken",
    }
    BatchWorksGone { pt: "algumas das obras não existem mais", en: "some of the works are not there" }
    MergeNotAPair { pt: "uma obra não forma um par consigo mesma", en: "a work is not a pair" }
    MergeNothing { pt: "não há nada para fundir na obra", en: "there is nothing to merge into the work" }

    // --- The library: what is not there (any more) ---
    NoWork { pt: "a obra {id} não existe", en: "there is no work {id}" }
    NoCategory { pt: "a categoria {id} não existe", en: "there is no category {id}" }
    NoTag { pt: "a etiqueta {id} não existe", en: "there is no tag {id}" }
    NoCollection { pt: "a coleção {id} não existe", en: "there is no collection {id}" }
    NoSmartCollection { pt: "a coleção inteligente {id} não existe", en: "there is no smart collection {id}" }
    NoField { pt: "o campo {id} não existe", en: "there is no field {id}" }
    NoDocument { pt: "o documento {id} não existe", en: "there is no document {id}" }
    NoSuggestion { pt: "a sugestão {id} não existe", en: "there is no suggestion {id}" }
    NoOpenSuggestion { pt: "a sugestão {id} não está mais em aberto", en: "there is no open suggestion {id}" }
    NoRoot { pt: "a pasta acompanhada {id} não existe", en: "there is no root {id}" }
    NoFile { pt: "o arquivo {id} não existe", en: "there is no file {id}" }
    NoFolderId { pt: "a pasta {id} não existe", en: "there is no folder {id}" }
    NoFolderAt { pt: "a pasta {path} não existe", en: "there is no folder {path}" }
    NoLibraryAt { pt: "não há biblioteca em {path}", en: "there is no library at {path}" }
    NoOperation { pt: "a operação {id} não existe", en: "there is no operation {id}" }

    // --- The library: annotations ---
    NoteTooLong { pt: "a nota é longa demais", en: "the note is too long" }
    NoAnnotation { pt: "essa anotação não existe", en: "there is no such annotation" }
    ImportTooMany { pt: "anotações demais para uma importação só", en: "too many annotations for one import" }
    ImportOtherDocument {
        pt: "uma anotação trazida é de outro documento",
        en: "an imported annotation is of another document",
    }
    ImportNeedsName {
        pt: "uma anotação trazida precisa de um nome que a distinga",
        en: "an imported annotation needs a name to be told apart",
    }

    // --- The library: a JSON-LD file of annotations brought in ---
    NotABlake3Name {
        pt: "{iri} não é o nome de um arquivo (urn:blake3: e 64 dígitos)",
        en: "{iri} is not a file's name (urn:blake3: and 64 digits)",
    }
    SelectorBad { pt: "um seletor {kind} {why}", en: "a {kind} selector {why}" }
    SelectorWithout { pt: "sem {name}", en: "without {name}" }
    SelectorTakesNo { pt: "não leva {name}", en: "does not take {name}" }
    SelectorPdfPageOnly { pt: "só pode dizer a página de um PDF", en: "can only name a PDF's page" }
    SelectorUtf16 {
        pt: "conta em unidades de UTF-16 (musaeum:unit)",
        en: "counts in UTF-16 units (musaeum:unit)",
    }
    SelectorNoStartEnd { pt: "sem start e end", en: "without start and end" }
    SelectorNoEnds { pt: "sem as duas pontas", en: "without both ends" }
    SelectorNotCfi { pt: "tem pontas que não são CFIs de EPUB", en: "has ends that are not EPUB CFIs" }
    SelectorTooDeep { pt: "aninhado fundo demais", en: "nested too deep" }
    SelectorPriority { pt: "com uma prioridade fora do lugar", en: "with a priority out of place" }
    SelectorUnknownKind { pt: "um seletor de tipo desconhecido: {name}", en: "a selector of an unknown kind: {name}" }
    NotADate { pt: "{text} não é uma data", en: "{text} is not a date" }
    SourceNotText {
        pt: "a fonte de uma anotação é de tipo {kind}, e não Text",
        en: "an annotation's source is of kind {kind}, not Text",
    }
    NotALayout { pt: "{layout} não é um layout", en: "{layout} is not a layout" }
    WorkTextTooLong {
        pt: "um título, um nome ou um identificador longo demais",
        en: "a title, a name or an identifier too long",
    }
    WorkTooManyPeople {
        pt: "pessoas ou identificadores demais numa obra",
        en: "too many people or identifiers in a work",
    }
    PlainNameTooLong { pt: "um nome longo demais", en: "a name too long" }
    IdentifierTooLong { pt: "um identificador longo demais", en: "an identifier too long" }
    ItemNotAnnotation { pt: "um item de tipo {kind}, e não Annotation", en: "an item of kind {kind}, not Annotation" }
    NotUrnUuid { pt: "{id} não é urn:uuid:", en: "{id} is not urn:uuid:" }
    MotivationMismatch {
        pt: "a anotação {uuid} diz {said}, e uma de tipo {mark} é {motivation}",
        en: "the annotation {uuid} says {said}, and one of kind {mark} is {motivation}",
    }
    NoteNotMarkdown {
        pt: "a nota da anotação {uuid} não é um TextualBody em text/markdown",
        en: "the note of the annotation {uuid} is not a TextualBody in text/markdown",
    }
    TargetCount {
        pt: "a anotação {uuid} tem alvos de menos ou demais",
        en: "the annotation {uuid} has too few or too many targets",
    }
    TargetKind { pt: "um alvo de tipo {kind}", en: "a target of kind {kind}" }
    TwoWorks { pt: "a anotação {uuid} está em duas obras", en: "the annotation {uuid} is in two works" }
    WholeBookHasPlace {
        pt: "a anotação {uuid} sobre o livro todo tem um lugar",
        en: "the annotation {uuid} about the whole book has a place",
    }
    SegmentNoOrdinal {
        pt: "um trecho da anotação {uuid} sem musaeum:ordinal",
        en: "a segment of the annotation {uuid} without musaeum:ordinal",
    }
    SegmentTooManySelectors { pt: "seletores demais num trecho", en: "too many selectors in a segment" }
    SegmentTwoPages {
        pt: "um trecho da anotação {uuid} diz duas páginas",
        en: "a segment of the annotation {uuid} names two pages",
    }
    SegmentNoState {
        pt: "um trecho da anotação {uuid} sem estado",
        en: "a segment of the annotation {uuid} without a state",
    }
    ColorOrPdfName {
        pt: "a cor ou o nome PDF da anotação {uuid}",
        en: "the color or the PDF name of the annotation {uuid}",
    }
    PositionNotNumber { pt: "uma posição que não é um número", en: "a position that is not a number" }
    FileTooDeep {
        pt: "o arquivo aninha objetos mais fundo que {max} níveis",
        en: "the file nests objects deeper than {max} levels",
    }
    NotMusaeumJsonLd {
        pt: "o arquivo não é um JSON-LD de anotações do Musaeum: {error}",
        en: "the file is not a Musaeum annotations JSON-LD: {error}",
    }
    ContextNotOurs {
        pt: "o @context não é o do Musaeum: nenhum outro é lido, e nenhum é buscado na rede",
        en: "the @context is not Musaeum's: no other is read, and none is fetched from the network",
    }
    NotACollection {
        pt: "o arquivo não é uma AnnotationCollection com uma página",
        en: "the file is not an AnnotationCollection with one page",
    }
    FormatVersion {
        pt: "o arquivo é da versão {found} do formato, e esta lê a {known}",
        en: "the file is of version {found} of the format, and this one reads {known}",
    }
    FileSaysNothing { pt: "o arquivo não diz o que contém", en: "the file does not say what it holds" }
    CountMismatch {
        pt: "o número de anotações do arquivo não confere",
        en: "the file's number of annotations does not add up",
    }

    CollectionLabel { pt: "Anotações do Musaeum", en: "Musaeum annotations" }

    // --- The library: Calibre ---
    Untitled { pt: "Sem título", en: "Untitled" }
    CalibreWriting {
        pt: "o Calibre está gravando nesta biblioteca (feche-o e tente de novo)",
        en: "Calibre is writing to this library (close it and try again)",
    }
    CalibreNothing {
        pt: "{path} não tem metadata.db nem livros com metadata.opf",
        en: "{path} has neither a metadata.db nor books with a metadata.opf",
    }
    CalibreNoBooksTable {
        pt: "o metadata.db não tem a tabela books: não é uma biblioteca do Calibre",
        en: "metadata.db has no books table: it is not a Calibre library",
    }
    CalibreTooMany { pt: "no máximo {max} livros de uma vez", en: "{max} books at a time at most" }

    // --- The library: its folders, and opening it ---
    NoLocalAppData {
        pt: "LOCALAPPDATA não está definido: indique a pasta da biblioteca",
        en: "LOCALAPPDATA is not set, so give a library folder explicitly",
    }
    NotAFolder { pt: "{path} não é uma pasta", en: "{path} is not a folder" }
    RootOverlaps {
        pt: "{path} se sobrepõe à pasta acompanhada {root}",
        en: "{path} overlaps the root at {root}",
    }
    RootHoldsLibrary {
        pt: "{path} contém os arquivos da própria biblioteca, ou está dentro deles ({library}): escolha uma pasta só de livros",
        en: "{path} holds the library's own files, or is inside them ({library}): choose a folder of books only",
    }
    DocumentNoFile {
        pt: "o documento {id} não tem arquivo disponível para ler",
        en: "document {id} has no file available to read",
    }
    LockedByAnotherProcess {
        pt: "outro Musaeum já está gravando nesta biblioteca",
        en: "another Musaeum is already writing to this library",
    }
    NotOurDatabase { pt: "{path} não é um banco de dados do Musaeum", en: "{path} is not a Musaeum database" }
    NewerSchema {
        pt: "esta biblioteca tem o esquema {found}, e este programa só conhece até o {known}",
        en: "this library has schema {found}, and this program only knows up to {known}",
    }
    NeedsUpgrade {
        pt: "esta biblioteca precisa de uma atualização, que só se faz abrindo-a para gravar",
        en: "this library needs an upgrade, which takes opening it for writing",
    }
    OpenedReadOnly {
        pt: "esta biblioteca foi aberta só para leitura",
        en: "this library was opened for reading only",
    }

    // --- The library: moving, renaming, deleting files, and undoing it ---
    FileNotThere {
        pt: "{path} não está lá, até onde a biblioteca sabe",
        en: "{path} is not there, as far as the library knows",
    }
    CannotLookAt { pt: "{path} não pôde ser examinado: {error}", en: "{path} cannot be looked at: {error}" }
    NotTheKnownFile {
        pt: "{path} não é o arquivo que a biblioteca conhece: falta reconciliar primeiro",
        en: "{path} is not the file the library knows: it needs a reconcile first",
    }
    TargetExists { pt: "{path} já existe", en: "{path} already exists" }
    TargetOtherVolume {
        pt: "{path} está em outro volume, o que exigiria uma cópia",
        en: "{path} is on another volume, which takes a copy",
    }
    MovingFailed { pt: "ao mover para {path}: {error}", en: "moving to {path}: {error}" }
    NotInsideRoot {
        pt: "{path} não é uma pasta dentro de uma pasta acompanhada",
        en: "{path} is not a folder inside a monitored root",
    }
    TargetNoName { pt: "o destino não tem nome de arquivo", en: "the target has no file name" }
    NameHidesFile {
        pt: "com o nome {name}, a biblioteca deixaria de ver este arquivo",
        en: "with the name {name}, the library would no longer see this file",
    }
    RootFolderMoved {
        pt: "a pasta de uma pasta acompanhada muda de lugar mudando a pasta acompanhada",
        en: "a root's own folder is moved by relocating the root",
    }
    FolderIntoItself { pt: "uma pasta não pode ir para dentro de si mesma", en: "a folder cannot move into itself" }
    NoRecycleBin {
        pt: "{path} está num volume sem Lixeira, onde excluir não se desfaz",
        en: "{path} is on a volume with no Recycle Bin, where deleting cannot be undone",
    }
    PathTooLongForBin {
        pt: "{path} tem um caminho longo demais para a Lixeira devolver, e fica onde está",
        en: "{path} has a path too long for the Recycle Bin to give back, so it stays where it is",
    }
    NameNotUnicode {
        pt: "{path} tem um nome que não é Unicode válido, sobre o qual a Lixeira não pode ser consultada",
        en: "{path} has a name that is not valid Unicode, which the Recycle Bin cannot be asked about",
    }
    BinUnreadable {
        pt: "a Lixeira não pôde ser lida, e isto não poderia ser desfeito: {error}",
        en: "the Recycle Bin cannot be read, so this could not be undone: {error}",
    }
    NothingToUndo {
        pt: "a operação {id} está {state}, e não há o que desfazer",
        en: "operation {id} is {state}, so there is nothing to undo",
    }
    UndoOfUndo { pt: "a operação {id} já é um desfazer", en: "operation {id} is itself an undo" }
    CannotUndoKind { pt: "uma operação {kind} não se desfaz", en: "a {kind} cannot be undone" }
    JournalNoFile { pt: "o registro da operação não diz o arquivo", en: "the journal entry names no file" }
    JournalNoFolder { pt: "o registro da operação não diz a pasta", en: "the journal entry names no folder" }
    FormerFolderGone {
        pt: "a pasta onde {path} estava não existe mais",
        en: "the folder {path} used to be in is gone",
    }
    RestoreWouldOverwrite {
        pt: "{path} já existe, e restaurar o sobrescreveria",
        en: "{path} already exists, and restoring would overwrite it",
    }
    NotInBin { pt: "{path} não está mais na Lixeira", en: "{path} is no longer in the Recycle Bin" }

    // --- Study reports, in Markdown and PDF (the kinds of mark are the subjects above) ---
    ReportTitle { pt: "Relatório de estudo", en: "Study report" }
    ReportMadeOne {
        pt: "Relatório de estudo gerado em {date} ({zone}) pelo {generator}.",
        en: "Study report made on {date} ({zone}) by {generator}.",
    }
    ReportMadeMany {
        pt: "{count} obras, gerado em {date} ({zone}) pelo {generator}.",
        en: "{count} works, made on {date} ({zone}) by {generator}.",
    }
    ReportAnnotator { pt: "Anotações de {name}.", en: "Annotations by {name}." }
    ReportPageOf { pt: "página {number} de {total}", en: "page {number} of {total}" }
    ReportDate { pt: "{day}/{month}/{year}", en: "{year}-{month}-{day}" }
    ReportDateTime {
        pt: "{day}/{month}/{year} às {hour}:{minute}",
        en: "{year}-{month}-{day} at {hour}:{minute}",
    }
    ReportSeries { pt: "Série", en: "Series" }
    ReportSeriesNumber { pt: ", n.º {number}", en: ", no. {number}" }
    ReportYear { pt: "Ano", en: "Year" }
    ReportLanguage { pt: "Idioma", en: "Language" }
    ReportIdentifiers { pt: "Identificadores", en: "Identifiers" }
    ReportFile { pt: "Arquivo", en: "File" }
    ReportAnnotations { pt: "Anotações", en: "Annotations" }
    ReportPagesOne { pt: "{format}, {count} página", en: "{format}, {count} page" }
    ReportPagesOther { pt: "{format}, {count} páginas", en: "{format}, {count} pages" }
    ReportNotFoundOne { pt: "{count} não encontrada", en: "{count} not found" }
    ReportNotFoundOther { pt: "{count} não encontradas", en: "{count} not found" }
    ReportWasAt { pt: "estava em {place}", en: "was at {place}" }
    ReportApproximate { pt: "aproximada", en: "approximate" }
    ReportPlacePage { pt: "p. {page}", en: "p. {page}" }
    ReportPlacePercent { pt: "{percent} %", en: "{percent}%" }
    ReportGraphicMark { pt: "Sem texto: uma marca desenhada na página.", en: "No text: a mark drawn on the page." }
    ReportNoAnnotationsInWork { pt: "Nenhuma anotação nesta obra.", en: "No annotations in this work." }
    ReportWholeBook { pt: "Sobre o livro todo", en: "About the whole book" }
    ReportNotFoundHeading { pt: "Não encontradas no livro", en: "Not found in the book" }
    ReportNotFoundExplained {
        pt: "O trecho destas anotações não foi encontrado no livro como ele está agora. Elas ficam com a citação, a nota e o lugar em que estavam.",
        en: "The passage of these annotations was not found in the book as it is now. They keep their quote, their note and the place where they were.",
    }
    ReportMarkBox { pt: "Caixa", en: "Box" }
    ReportMarkStroke { pt: "Traço", en: "Stroke" }
    ReportMarkPoint { pt: "Nota fixada", en: "Pinned note" }
    ReportNone { pt: "nenhuma anotação", en: "no annotations" }
    ReportListLast { pt: "{rest} e {last}", en: "{rest} and {last}" }
    ReportHighlightsOne { pt: "{count} realce", en: "{count} highlight" }
    ReportHighlightsOther { pt: "{count} realces", en: "{count} highlights" }
    ReportUnderlinesOne { pt: "{count} sublinhado", en: "{count} underline" }
    ReportUnderlinesOther { pt: "{count} sublinhados", en: "{count} underlines" }
    ReportNotesOne { pt: "{count} nota", en: "{count} note" }
    ReportNotesOther { pt: "{count} notas", en: "{count} notes" }
    ReportBoxesOne { pt: "{count} caixa", en: "{count} box" }
    ReportBoxesOther { pt: "{count} caixas", en: "{count} boxes" }
    ReportStrokesOne { pt: "{count} traço", en: "{count} stroke" }
    ReportStrokesOther { pt: "{count} traços", en: "{count} strokes" }
    ReportPinnedOne { pt: "{count} nota fixada", en: "{count} pinned note" }
    ReportPinnedOther { pt: "{count} notas fixadas", en: "{count} pinned notes" }
    RoleEditorShort { pt: "org.", en: "ed." }
    RoleTranslatorShort { pt: "trad.", en: "trans." }
    RoleIllustratorShort { pt: "il.", en: "ill." }
    ColorYellow { pt: "amarelo", en: "yellow" }
    ColorGreen { pt: "verde", en: "green" }
    ColorBlue { pt: "azul", en: "blue" }
    ColorPink { pt: "rosa", en: "pink" }
    ColorPurple { pt: "roxo", en: "purple" }

    // --- Links out of a book ---
    LinkNotWeb {
        pt: "Este endereço não é um endereço da web que o Musaeum abra.",
        en: "This address is not a web address Musaeum opens.",
    }
    BrowserDidNotOpenCode { pt: "O navegador não abriu (código {code}).", en: "The browser did not open (code {code})." }
    BrowserDidNotOpen { pt: "O navegador não abriu.", en: "The browser did not open." }
    BrowserWindowsOnly {
        pt: "Abrir no navegador ainda só existe no Windows.",
        en: "Opening in the browser exists only on Windows so far.",
    }
}
