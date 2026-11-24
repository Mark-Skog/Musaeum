/**
 * Bringing a Calibre library in: its folder, chosen with the system's dialog; what it holds, read
 * without writing to it; and the import, which follows the folder and brings each book in as the
 * library reads its files.
 */

import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';
import { Button } from 'react-aria-components';

import { api, type CalibrePreview, type CalibreReport } from '../api';
import { type MessageKey, tn, useT } from '../i18n';
import { Frame, reasonOf } from './dialogs';

const KINDS: Record<string, MessageKey> = {
  text: 'batch.calibre.kind.text',
  number: 'batch.calibre.kind.number',
  date: 'batch.calibre.kind.date',
  boolean: 'batch.calibre.kind.boolean',
  choice: 'batch.calibre.kind.choice',
  url: 'batch.calibre.kind.url',
};

export interface CalibreDialogProps {
  /** A folder already chosen, as a test gives it; otherwise the person chooses one. */
  folder: string | null;
  onClose(): void;
  onImported(report: CalibreReport): void;
}

export function CalibreDialog({ folder, onClose, onImported }: CalibreDialogProps): React.JSX.Element {
  const t = useT();
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
    <Frame title={t('batch.calibre.title')} onClose={onClose} onSubmit={submit} submitLabel={done ? t('common.close') : t('batch.calibre.import')} error={error} busy={busy || (!done && preview === null)} wide>
      <div className="calibre-dialog">
        <div className="row">
          <span className="grow calibre-path" title={path ?? ''}>
            {path ?? t('batch.calibre.choosePrompt')}
          </span>
          <Button className="button" isDisabled={busy} onPress={choose}>
            {t('batch.calibre.choose')}
          </Button>
        </div>
        {preview !== null ? (
          <div className="calibre-preview" data-books={preview.books}>
            <p>
              {tn('batch.calibre.books', preview.books)}, {tn('batch.calibre.files', preview.files)}
              {preview.missing > 0 ? `; ${t('batch.calibre.missingOnDisk', { count: preview.missing })}` : ''}. {t('batch.calibre.readFrom', { source: preview.readFrom })}
            </p>
            {preview.sample.length > 0 ? <p className="muted">{preview.sample.join(' · ')}</p> : null}
            {preview.columns.length > 0 ? (
              <ul className="calibre-columns" aria-label={t('batch.calibre.columns')}>
                {preview.columns.map((column) => (
                  <li key={column.label}>
                    {column.name}: {column.kind === null ? (
                      <span className="muted">{t('batch.calibre.column.notBrought')}</span>
                    ) : (
                      t('batch.calibre.column.fieldOf', { kind: column.kind in KINDS ? t(KINDS[column.kind]) : column.kind })
                    )}
                  </li>
                ))}
              </ul>
            ) : null}
            <p className="muted">
              {t('batch.calibre.readOnly')} {preview.followed ? t('batch.calibre.followed') : t('batch.calibre.willFollow')} {t('batch.calibre.sameBytes')}
            </p>
          </div>
        ) : null}
        {report !== null ? (
          <p className="calibre-report" role="status" data-imported={report.imported} data-waiting={report.waiting}>
            {t(done ? 'batch.calibre.report.done' : 'batch.calibre.report.sofar', { imported: report.imported, books: report.books })}
            {report.waiting > 0 ? `; ${t('batch.calibre.report.waiting', { count: report.waiting })}` : ''}
            {report.missing > 0 ? `; ${t('batch.calibre.report.missing', { count: report.missing })}` : ''}
            {report.merged > 0 ? `; ${t('batch.calibre.report.merged', { count: report.merged })}` : ''}
            {report.fieldsCreated > 0 ? `; ${t('batch.calibre.report.fields', { count: report.fieldsCreated })}` : ''}.
          </p>
        ) : null}
      </div>
    </Frame>
  );
}
