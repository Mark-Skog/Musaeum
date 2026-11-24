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
import { tn, useT } from '../i18n';
import { reasonOf } from './dialogs';
import { Chips, Picker } from './Inspector';
import { type BatchDraft, batchEditOf, categoryPath, emptyBatch, statuses } from './taxonomy-model';

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
  const t = useT();
  const VARIOUS = t('batch.various');
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
      <aside className="inspector batch" aria-label={t('batch.label')} aria-busy={error === null}>
        <p className="muted">{error ?? t('details.reading')}</p>
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
  const share = (works: number): string => (works === summary.works ? t('batch.inAll') : t('batch.inSome', { count: works, total: summary.works }));
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
      <ul className="shared-list" aria-label={t('batch.sharedOf', { label })}>
        {shared.map((item) => {
          const taken = draft[field].includes(item.id);
          return (
            <li key={item.id} className={taken ? 'shared taken' : 'shared'} data-shared={name(item.id)}>
              <span className="shared-name">{name(item.id)}</span>
              <span className="muted">{share(item.works)}</span>
              <Button className="button small" aria-label={taken ? t('batch.keep', { name: name(item.id) }) : t('batch.removeFromAll', { name: name(item.id) })} onPress={() => toggle(field, item.id)}>
                {taken ? '↺' : '×'}
              </Button>
            </li>
          );
        })}
      </ul>
    );

  return (
    <aside className="inspector batch" aria-label={t('batch.label')} aria-busy={saving} data-works={summary.works}>
      <header className="inspector-head">
        <h2>{t('batch.books', { count: books })}</h2>
        <Button className="button small" aria-label={t('details.close')} onPress={onClose}>
          ×
        </Button>
      </header>
      <div className="inspector-body">
        <p className="muted">
          {t('batch.intro')} {summary.works < books ? t('batch.works', { count: summary.works }) : ''}
        </p>

        <TextField className="field" value={value('creators')} onChange={(creators) => set('creators', creators)}>
          <Label>{t('batch.creators')}</Label>
          <Input className="text-input" placeholder={placeholder('creators')} data-field="batch-creators" />
        </TextField>

        <TextField className="field" value={value('series')} onChange={(series) => set('series', series)}>
          <Label>{t('details.series')}</Label>
          <Input className="text-input" placeholder={placeholder('series')} data-field="batch-series" />
        </TextField>

        <div className="row">
          <TextField className="field narrow" value={value('year')} onChange={(year) => set('year', year)}>
            <Label>{t('details.year')}</Label>
            <Input className="text-input" inputMode="numeric" placeholder={placeholder('year')} data-field="batch-year" />
          </TextField>
          <TextField className="field grow" value={value('language')} onChange={(language) => set('language', language)}>
            <Label>{t('details.language')}</Label>
            <Input className="text-input" placeholder={placeholder('language') ?? 'pt-BR'} data-field="batch-language" />
          </TextField>
        </div>

        <div className="row">
          <div className="field" role="group" aria-label={t('details.rating')}>
            <span className="field-label">
              {t('details.rating')}{' '}
              {draft.rating === undefined && !('rating' in summary.same) ? <span className="muted">{VARIOUS}</span> : null}
            </span>
            <div className="stars">
              {[1, 2, 3, 4, 5].map((stars) => (
                <ToggleButton
                  key={stars}
                  className="star"
                  isSelected={rating !== null && stars <= rating}
                  aria-label={tn('details.stars', stars)}
                  onChange={() => set('rating', rating === stars ? null : stars)}
                >
                  ★
                </ToggleButton>
              ))}
            </div>
          </div>
          <div className="field" role="group" aria-label={t('details.statusGroup')}>
            <span className="field-label">{t('details.status')}</span>
            <Picker
              label={t('details.statusGroup')}
              value={status}
              options={status === '' ? [['', VARIOUS], ...statuses()] : statuses()}
              onChange={(chosen) => chosen !== '' && set('status', chosen)}
            />
          </div>
        </div>

        <div className="field" role="group" aria-label={t('details.categories')}>
          <span className="field-label">{t('details.categories')}</span>
          {sharedList(t('details.categories'), summary.categories, categoryName, 'removeCategories')}
          <Chips
            label={t('details.categories')}
            chosen={draft.addCategories.map((id) => ({ key: String(id), text: categoryName(id) }))}
            options={taxonomy.categories.map((category) => ({ key: String(category.id), text: categoryName(category.id) }))}
            onChange={(keys) => set('addCategories', keys.map(Number))}
          />
        </div>

        <div className="field" role="group" aria-label={t('details.tags')}>
          <span className="field-label">{t('details.tags')}</span>
          {sharedList(t('details.tags'), summary.tags, tagName, 'removeTags')}
          <Chips
            label={t('details.tags')}
            chosen={draft.addTags.map((name) => ({ key: name, text: name }))}
            options={taxonomy.tags.map((tag) => ({ key: tag.name, text: tag.name }))}
            allowsNew
            onChange={(names) => set('addTags', names)}
          />
        </div>

        <div className="field" role="group" aria-label={t('details.collections')}>
          <span className="field-label">{t('details.collections')}</span>
          {sharedList(t('details.collections'), summary.collections, collectionName, 'removeCollections')}
          <Chips
            label={t('details.collections')}
            chosen={draft.addCollections.map((id) => ({ key: String(id), text: collectionName(id) }))}
            options={taxonomy.collections.map((collection) => ({ key: String(collection.id), text: collection.name }))}
            onChange={(keys) => set('addCollections', keys.map(Number))}
          />
        </div>

        {summary.works >= 2 ? (
          <Button className="button" onPress={() => onMerge(works)}>
            {t('batch.merge')}
          </Button>
        ) : null}
        <ExchangeSection
          heading={t('batch.annotations')}
          controls={exchange}
          actions={['export-jsonld', 'report-md', 'report-pdf']}
          id="batch-exchange"
          className="inspector-exchange"
          idle={t('batch.annotations.idle')}
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
            {t('details.discard')}
          </Button>
          <Button id="save-works" className="button primary" isDisabled={!changed || saving || problems.length > 0} onPress={save}>
            {t('batch.saveIn', { count: books })}
          </Button>
        </div>
      </footer>
    </aside>
  );
}
