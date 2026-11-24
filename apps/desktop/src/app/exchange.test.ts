import { describe, expect, it } from 'vitest';

import { exportedText, importedText, reportText } from './exchange';

describe('what the annotations in and out say they did', () => {
  it('counts an import by how each one was found, what was there already and what is missing', () => {
    expect(
      importedText({
        path: 'C:\\x.jsonld',
        total: 12,
        added: 9,
        already: 1,
        elsewhere: 0,
        otherWork: 0,
        unmatched: [{ title: 'Outro livro', creators: ['Alguém'], annotations: 2 }],
        books: [
          { documentId: 1, title: 'A', added: 6, exact: 6, approximate: 0, orphan: 0, pending: 0 },
          { documentId: 2, title: 'B', added: 3, exact: 1, approximate: 1, orphan: 1, pending: 0 },
        ],
      }),
    ).toBe('9 anotações trazidas (7 exatas, 1 aproximada, 1 órfã); 1 já estava na biblioteca; 2 são de obras que a biblioteca não tem (“Outro livro”).');
    expect(
      importedText({ path: '', total: 2, added: 0, already: 2, elsewhere: 0, otherWork: 0, unmatched: [], books: [] }),
    ).toBe('0 anotações trazidas; 2 já estavam na biblioteca.');
    expect(
      importedText({
        path: '',
        total: 3,
        added: 2,
        already: 0,
        elsewhere: 1,
        otherWork: 2,
        unmatched: [],
        books: [{ documentId: 1, title: 'A', added: 2, exact: 0, approximate: 0, orphan: 0, pending: 2 }],
      }),
    ).toBe(
      '2 anotações trazidas (2 a ancorar quando o livro abrir); 1 é de outro livro e ficou de fora. 2 vieram de outra obra, e foram trazidas porque este livro foi escolhido: confira.',
    );
  });

  it('says where an export and a report were written', () => {
    expect(exportedText({ path: 'D:\\a.jsonld', annotations: 1, documents: 1, works: 1, bytes: 10 })).toBe('JSON-LD com 1 anotação salvo em D:\\a.jsonld.');
    expect(exportedText({ path: 'D:\\a.jsonld', annotations: 30, documents: 4, works: 3, bytes: 10 })).toBe('JSON-LD com 30 anotações de 4 livros salvo em D:\\a.jsonld.');
    expect(reportText({ path: 'D:\\r.pdf', works: 1, annotations: 7, pages: 3, bytes: 9 }, 'pdf')).toBe('Relatório (PDF, 3 páginas) com 7 anotações salvo em D:\\r.pdf.');
    expect(reportText({ path: 'D:\\r.md', works: 2, annotations: 1, pages: null, bytes: 9 }, 'markdown')).toBe('Relatório (Markdown) com 1 anotação de 2 livros salvo em D:\\r.md.');
  });
});
