/**
 * The details of many books at once: what they have in common, and what to change on all of them.
 * A field the books differ on shows "(vários)" until the person types into it; only the fields
 * touched are saved, on every book, in one change that the host makes whole or not at all, and
 * each becomes the person's. Categories, tags and collections show how many of the books are in
 * each, and are added to or taken from all of them.
 */

import { useEffect, useState } from 'react';
import { Button, Input, Label, TextField, ToggleButton } from 'react-aria-components';

import { api, type BatchSummary, type Credit, type Taxonomy } from '../api';
import { ExchangeSection, useExchange } from '../ExchangeSection';
import { reasonOf } from './dialogs';
import { Chips, Picker } from './Inspector';
import { type BatchDraft, batchEditOf, categoryPath, emptyBatch, STATUSES } from './taxonomy-model';

export interface BatchPanelProps {
  /** The works chosen, each once. */
  works: number[];
  /** How many books (documents) were chosen. */
  books: number;
  taxonomy: Taxonomy;
  onSaved(): void;
  onClose(): void;
  onMerge(works: number[]): void;
}

const VARIOUS = '(vários)';

function shownText(summary: BatchSummary, field: keyof BatchSummary['same']): string {
  if (!(field in summary.same)) {
    return '';
  }
  const value = summary.same[field];
  if (value === null || value === undefined) {
    return '';
  }
  if (field === 'creators') {
    return (value as Credit[]).map((credit) => credit.name).join('; ');
  }
  return String(value);
}

export function BatchPanel({ works, books, taxonomy, onSaved, onClose, onMerge }: BatchPanelProps): React.JSX.Element {
  const [summary, setSummary] = useState<BatchSummary | null>(null);
  const [draft, setDraft] = useState<BatchDraft>(emptyBatch);
  const [error, setError] = useState<string | null>(null);
  const exchange = useExchange({ kind: 'works', works }, undefined, 'batch');
  const [saving, setSaving] = useState(false);
  const [version, setVersion] = useState(0);
  const key = works.join(',');

  useEffect(() => {
    let current = true;
    setError(null);
    api
      .worksSummary(works)
      .then((loaded) => current && setSummary(loaded))
      .catch((reason: unknown) => current && setError(reasonOf(reason)));
    return () => {
      current = false;
    };
    // `key` stands for `works`, which is a new array on every render.
  }, [key, version]);
  useEffect(() => setDraft(emptyBatch()), [key]);

  if (summary === null) {
    return (
      <aside className="inspector batch" aria-label="Detalhes de vários livros">
        <p className="muted">{error ?? 'Lendo…'}</p>
      </aside>
    );
  }
  const { edit, problems, changed } = batchEditOf(draft);
  const set = <K extends keyof BatchDraft>(field: K, value: BatchDraft[K]): void => setDraft({ ...draft, [field]: value });
  const placeholder = (field: keyof BatchSummary['same']): string | undefined => (field in summary.same ? undefined : VARIOUS);
  const value = (field: 'creators' | 'series' | 'year' | 'language'): string => draft[field] ?? shownText(summary, field);
  const rating = draft.rating !== undefined ? draft.rating : 'rating' in summary.same ? ((summary.same.rating as number | null) ?? null) : null;
  const status = draft.status ?? ('status' in summary.same ? String(summary.same.status) : '');
  const toggle = (field: 'removeCategories' | 'removeTags' | 'removeCollections', id: number): void => {
    const list = draft[field];
    set(field, list.includes(id) ? list.filter((item) => item !== id) : [...list, id]);
  };
  const share = (works: number): string => (works === summary.works ? 'em todos' : `em ${works} de ${summary.works}`);
  const save = (): void => {
    if (!changed || problems.length > 0) {
      return;
    }
    setSaving(true);
    setError(null);
    api
      .editWorks(works, edit)
      .then(() => {
        setDraft(emptyBatch());
        setVersion((value) => value + 1);
        onSaved();
      })
      .catch((reason: unknown) => setError(reasonOf(reason)))
      .finally(() => setSaving(false));
  };
  const categoryName = (id: number): string => categoryPath(taxonomy.categories, id);
  const tagName = (id: number): string => taxonomy.tags.find((tag) => tag.id === id)?.name ?? String(id);
  const collectionName = (id: number): string => taxonomy.collections.find((collection) => collection.id === id)?.name ?? String(id);

  const sharedList = (
    label: string,
    shared: BatchSummary['categories'],
    name: (id: number) => string,
    field: 'removeCategories' | 'removeTags' | 'removeCollections',
  ): React.JSX.Element | null =>
    shared.length === 0 ? null : (
      <ul className="shared-list" aria-label={`${label} dos livros`}>
        {shared.map((item) => {
          const taken = draft[field].includes(item.id);
          return (
            <li key={item.id} className={taken ? 'shared taken' : 'shared'} data-shared={name(item.id)}>
              <span className="shared-name">{name(item.id)}</span>
              <span className="muted">{share(item.works)}</span>
              <Button className="button small" aria-label={taken ? `Manter ${name(item.id)}` : `Tirar ${name(item.id)} de todos`} onPress={() => toggle(field, item.id)}>
                {taken ? '↺' : '×'}
              </Button>
            </li>
          );
        })}
      </ul>
    );

  return (
    <aside className="inspector batch" aria-label="Detalhes de vários livros" aria-busy={saving} data-works={summary.works}>
      <header className="inspector-head">
        <h2>
          {books} livros
        </h2>
        <Button className="button small" aria-label="Fechar os detalhes" onPress={onClose}>
          ×
        </Button>
      </header>
      <div className="inspector-body">
        <p className="muted">O que mudar aqui muda em todos eles. {summary.works < books ? `São ${summary.works} obras.` : ''}</p>

        <TextField className="field" value={value('creators')} onChange={(creators) => set('creators', creators)}>
          <Label>Autores, separados por “;”</Label>
          <Input className="text-input" placeholder={placeholder('creators')} data-field="batch-creators" />
        </TextField>

        <TextField className="field" value={value('series')} onChange={(series) => set('series', series)}>
          <Label>Série</Label>
          <Input className="text-input" placeholder={placeholder('series')} data-field="batch-series" />
        </TextField>

        <div className="row">
          <TextField className="field narrow" value={value('year')} onChange={(year) => set('year', year)}>
            <Label>Ano</Label>
            <Input className="text-input" inputMode="numeric" placeholder={placeholder('year')} data-field="batch-year" />
          </TextField>
          <TextField className="field grow" value={value('language')} onChange={(language) => set('language', language)}>
            <Label>Idioma</Label>
            <Input className="text-input" placeholder={placeholder('language') ?? 'pt-BR'} data-field="batch-language" />
          </TextField>
        </div>

        <div className="row">
          <div className="field" role="group" aria-label="Avaliação">
            <span className="field-label">Avaliação {draft.rating === undefined && !('rating' in summary.same) ? <span className="muted">{VARIOUS}</span> : null}</span>
            <div className="stars">
              {[1, 2, 3, 4, 5].map((stars) => (
                <ToggleButton
                  key={stars}
                  className="star"
                  isSelected={rating !== null && stars <= rating}
                  aria-label={`${stars} ${stars === 1 ? 'estrela' : 'estrelas'}`}
                  onChange={() => set('rating', rating === stars ? null : stars)}
                >
                  ★
                </ToggleButton>
              ))}
            </div>
          </div>
          <div className="field" role="group" aria-label="Estado de leitura">
            <span className="field-label">Estado</span>
            <Picker
              label="Estado de leitura"
              value={status}
              options={status === '' ? [['', VARIOUS], ...STATUSES] : STATUSES}
              onChange={(chosen) => chosen !== '' && set('status', chosen)}
            />
          </div>
        </div>

        <div className="field" role="group" aria-label="Categorias">
          <span className="field-label">Categorias</span>
          {sharedList('Categorias', summary.categories, categoryName, 'removeCategories')}
          <Chips
            label="Categorias"
            chosen={draft.addCategories.map((id) => ({ key: String(id), text: categoryName(id) }))}
            options={taxonomy.categories.map((category) => ({ key: String(category.id), text: categoryName(category.id) }))}
            onChange={(keys) => set('addCategories', keys.map(Number))}
          />
        </div>

        <div className="field" role="group" aria-label="Etiquetas">
          <span className="field-label">Etiquetas</span>
          {sharedList('Etiquetas', summary.tags, tagName, 'removeTags')}
          <Chips
            label="Etiquetas"
            chosen={draft.addTags.map((name) => ({ key: name, text: name }))}
            options={taxonomy.tags.map((tag) => ({ key: tag.name, text: tag.name }))}
            allowsNew
            onChange={(names) => set('addTags', names)}
          />
        </div>

        <div className="field" role="group" aria-label="Coleções">
          <span className="field-label">Coleções</span>
          {sharedList('Coleções', summary.collections, collectionName, 'removeCollections')}
          <Chips
            label="Coleções"
            chosen={draft.addCollections.map((id) => ({ key: String(id), text: collectionName(id) }))}
            options={taxonomy.collections.map((collection) => ({ key: String(collection.id), text: collection.name }))}
            onChange={(keys) => set('addCollections', keys.map(Number))}
          />
        </div>

        {summary.works >= 2 ? (
          <Button className="button" onPress={() => onMerge(works)}>
            Fundir em um livro…
          </Button>
        ) : null}
        <ExchangeSection
          heading="Anotações dos escolhidos"
          controls={exchange}
          actions={['export-jsonld', 'report-md', 'report-pdf']}
          id="batch-exchange"
          className="inspector-exchange"
          idle="As anotações destes livros em um JSON-LD, ou num relatório de estudo só."
        />
      </div>
      <footer className="inspector-foot">
        {problems.length > 0 ? (
          <ul className="form-error" role="alert">
            {problems.map((problem) => (
              <li key={problem}>{problem}</li>
            ))}
          </ul>
        ) : null}
        {error !== null ? (
          <p className="form-error" role="alert">
            {error}
          </p>
        ) : null}
        <div className="modal-actions">
          <Button className="button" isDisabled={!changed || saving} onPress={() => setDraft(emptyBatch())}>
            Descartar
          </Button>
          <Button id="save-works" className="button primary" isDisabled={!changed || saving || problems.length > 0} onPress={save}>
            Salvar em {books} livros
          </Button>
        </div>
      </footer>
    </aside>
  );
}
