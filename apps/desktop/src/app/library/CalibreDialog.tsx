/**
 * Bringing a Calibre library in: its folder, chosen with the system's dialog; what it holds, read
 * without writing to it; and the import, which follows the folder and brings each book in as the
 * library reads its files.
 */

import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';
import { Button } from 'react-aria-components';

import { api, type CalibrePreview, type CalibreReport } from '../api';
import { Frame, reasonOf } from './dialogs';

const KINDS: Record<string, string> = {
  text: 'texto',
  number: 'número',
  date: 'data',
  boolean: 'sim/não',
  choice: 'lista de opções',
  url: 'endereço',
};

export interface CalibreDialogProps {
  /** A folder already chosen, as a test gives it; otherwise the person chooses one. */
  folder: string | null;
  onClose(): void;
  onImported(report: CalibreReport): void;
}

export function CalibreDialog({ folder, onClose, onImported }: CalibreDialogProps): React.JSX.Element {
  const [path, setPath] = useState<string | null>(folder);
  const [preview, setPreview] = useState<CalibrePreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<CalibreReport | null>(null);
  const [done, setDone] = useState(false);

  useEffect(() => {
    if (path === null) {
      return;
    }
    let current = true;
    setPreview(null);
    setError(null);
    api
      .calibrePreview(path)
      .then((loaded) => current && setPreview(loaded))
      .catch((reason: unknown) => current && setError(reasonOf(reason)));
    return () => {
      current = false;
    };
  }, [path]);
  useEffect(() => {
    const stop = listen<{ phase: string; report: CalibreReport | null }>('calibre-progress', (event) => {
      if (event.payload.report !== null) {
        setReport(event.payload.report);
      }
    });
    return () => {
      void stop.then((unlisten) => unlisten());
    };
  }, []);

  const choose = (): void => {
    api
      .calibreChoose()
      .then((chosen) => chosen !== null && setPath(chosen))
      .catch((reason: unknown) => setError(reasonOf(reason)));
  };
  const submit = (): void => {
    if (done) {
      onClose();
      return;
    }
    if (path === null || preview === null) {
      return;
    }
    setBusy(true);
    setError(null);
    api
      .calibreImport(path)
      .then((final) => {
        setReport(final);
        setDone(true);
        onImported(final);
      })
      .catch((reason: unknown) => setError(reasonOf(reason)))
      .finally(() => setBusy(false));
  };

  return (
    <Frame title="Importar do Calibre" onClose={onClose} onSubmit={submit} submitLabel={done ? 'Fechar' : 'Importar'} error={error} busy={busy || (!done && preview === null)} wide>
      <div className="calibre-dialog">
        <div className="row">
          <span className="grow calibre-path" title={path ?? ''}>
            {path ?? 'Escolha a pasta da biblioteca do Calibre (a que tem o metadata.db).'}
          </span>
          <Button className="button" isDisabled={busy} onPress={choose}>
            Escolher a pasta…
          </Button>
        </div>
        {preview !== null ? (
          <div className="calibre-preview" data-books={preview.books}>
            <p>
              {preview.books} {preview.books === 1 ? 'livro' : 'livros'}, {preview.files} {preview.files === 1 ? 'arquivo' : 'arquivos'}
              {preview.missing > 0 ? `; ${preview.missing} sem arquivo no disco` : ''}. Lido de {preview.readFrom}.
            </p>
            {preview.sample.length > 0 ? <p className="muted">{preview.sample.join(' · ')}</p> : null}
            {preview.columns.length > 0 ? (
              <ul className="calibre-columns" aria-label="Colunas personalizadas">
                {preview.columns.map((column) => (
                  <li key={column.label}>
                    {column.name}: {column.kind === null ? <span className="muted">não é trazida (calculada pelo Calibre)</span> : `campo de ${KINDS[column.kind] ?? column.kind}`}
                  </li>
                ))}
              </ul>
            ) : null}
            <p className="muted">
              A pasta do Calibre é só lida: nada nela muda, nem o metadata.db.{' '}
              {preview.followed ? 'A biblioteca já acompanha essa pasta.' : 'A biblioteca passa a acompanhar essa pasta, como as outras.'} Um livro com os mesmos bytes de um que a
              biblioteca já tem fica o mesmo livro.
            </p>
          </div>
        ) : null}
        {report !== null ? (
          <p className="calibre-report" role="status" data-imported={report.imported} data-waiting={report.waiting}>
            {done ? 'Importados' : 'Até agora'}: {report.imported} de {report.books}
            {report.waiting > 0 ? `; ${report.waiting} esperando a biblioteca ler os arquivos` : ''}
            {report.missing > 0 ? `; ${report.missing} sem arquivo` : ''}
            {report.merged > 0 ? `; ${report.merged} formatos juntados ao seu livro` : ''}
            {report.fieldsCreated > 0 ? `; ${report.fieldsCreated} campos personalizados criados` : ''}.
          </p>
        ) : null}
      </div>
    </Frame>
  );
}
