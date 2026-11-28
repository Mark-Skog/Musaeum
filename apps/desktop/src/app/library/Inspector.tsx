/**
 * The editor beside the books: the chosen book's work, as the library knows it (title, who made it
 * and in what role, series, year, language, identifiers, rating, reading status) and where the
 * person put it (categories, tags, collections, custom fields). Nothing is written until *Salvar*;
 * then only what changed, and every field changed becomes the person's: a later reading of the
 * file never writes over it. Beside each field, where its value came from.
 */

import { type ContextType, type RefObject, useContext, useEffect, useMemo, useRef, useState } from 'react';
import {
  Button,
  ComboBox,
  ComboBoxStateContext,
  Input,
  Label,
  ListBox,
  ListBoxItem,
  Popover,
  Select,
  SelectValue,
  Tag as Chip,
  TagGroup,
  TagList,
  TextArea,
  TextField,
  ToggleButton,
} from 'react-aria-components';

import { api, type BookRow, type CustomField, type Taxonomy, type WorkMetadata } from '../api';
import { ExchangeSection, useExchange } from '../ExchangeSection';
import { getLocale, tn, useT } from '../i18n';
import { ScrollFrame } from '../OverlayScrollbar';
import { reasonOf, Tick } from './dialogs';
import { categoryPath, type Draft, draftOf, editOf, matching, named, provenanceLabel, roles, schemes, statuses } from './taxonomy-model';

function Origin({ work, field }: { work: WorkMetadata; field: string }): React.JSX.Element | null {
  const label = provenanceLabel(work.provenance[field]);
  return label === null ? null : <span className={`origin origin-${work.provenance[field]}`}>{label}</span>;
}

export function Picker({ label, value, options, onChange, className }: { label: string; value: string; options: [string, string][]; onChange(value: string): void; className?: string }): React.JSX.Element {
  return (
    <Select className={className ?? 'picker'} selectedKey={value} onSelectionChange={(key) => onChange(String(key))} aria-label={label} data-picker={label}>
      <Button className="button select-button">
        <SelectValue />
        <span aria-hidden="true">▾</span>
      </Button>
      <Popover className="popover select-popover">
        <ListBox className="select-list">
          {options.map(([id, text]) => (
            <ListBoxItem key={id} id={id} className="select-item" textValue={text}>
              {text}
            </ListBoxItem>
          ))}
        </ListBox>
      </Popover>
    </Select>
  );
}

type ComboState = NonNullable<ContextType<typeof ComboBoxStateContext>>;

/** The box of `Chips`, inside its combo box: hands the combo box's state out, and opens the list
 * with everything left in it when the box is clicked. Not on focus: when the list closes, its
 * popover gives the focus back to the box, and that would open it again. */
function ChipsInput({ combo, placeholder, onKeyDown }: { combo: RefObject<ComboState | null>; placeholder: string; onKeyDown(event: React.KeyboardEvent): void }): React.JSX.Element {
  const state = useContext(ComboBoxStateContext);
  useEffect(() => {
    combo.current = state;
  });
  return <Input className="text-input" placeholder={placeholder} onClick={() => state?.open(null, 'manual')} onKeyDown={onKeyDown} />;
}

/** Chips of what a work is in, each with a ×, and a box to add one by name. */
export function Chips({
  label,
  chosen,
  options,
  allowsNew,
  onChange,
}: {
  label: string;
  chosen: { key: string; text: string }[];
  options: { key: string; text: string }[];
  allowsNew?: boolean;
  onChange(keys: string[]): void;
}): React.JSX.Element {
  const t = useT();
  const [typed, setTyped] = useState('');
  // The combo box's own state, handed out from inside it, to open and close its list.
  const combo = useRef<ComboState | null>(null);
  const left = options.filter((option) => !chosen.some((item) => item.key === option.key));
  // The items are given, so React Aria leaves filtering them by what is typed to us.
  const shown = matching(left, typed);
  const add = (key: string): void => {
    if (key.trim() !== '' && !chosen.some((item) => item.key === key)) {
      onChange([...chosen.map((item) => item.key), key]);
    }
    setTyped('');
    // Closed after each pick. React Aria closes the list when the selected key changes, which here
    // stays null, and opens it again when the text clears: it then covered the fields below, and a
    // click meant for one of them picked from it.
    combo.current?.close();
  };
  const enter = (event: React.KeyboardEvent): void => {
    const text = typed.trim();
    // An option highlighted with the arrows is the combo box's to pick.
    if (event.key !== 'Enter' || text === '' || combo.current?.selectionManager.focusedKey != null) {
      return;
    }
    const same = named(left, text);
    if (same !== undefined) {
      event.preventDefault();
      add(same.key);
    } else if (allowsNew) {
      event.preventDefault();
      add(text);
    }
  };
  return (
    <div className="chips-field">
      {chosen.length > 0 ? (
        <TagGroup aria-label={label} onRemove={(keys) => onChange(chosen.map((item) => item.key).filter((key) => !keys.has(key)))}>
          <TagList className="chips" items={chosen}>
            {(item) => (
              <Chip id={item.key} textValue={item.text} className="chip">
                {item.text}
                <Button slot="remove" className="chip-remove" aria-label={t('details.removeChip', { name: item.text })}>
                  ×
                </Button>
              </Chip>
            )}
          </TagList>
        </TagGroup>
      ) : null}
      <ComboBox
        aria-label={t('details.addChip', { what: label.toLocaleLowerCase(getLocale()) })}
        className="combo"
        inputValue={typed}
        onInputChange={(value) => {
          setTyped(value);
          if (value.trim() !== '') {
            combo.current?.open(null, 'input');
          }
        }}
        allowsCustomValue={allowsNew}
        selectedKey={null}
        onSelectionChange={(key) => key !== null && add(String(key))}
        // Opened here, never by React Aria by itself: on a click, on typing, and by the arrow keys.
        menuTrigger="manual"
        items={shown}
      >
        <ChipsInput combo={combo} placeholder={allowsNew ? t('details.chips.typeNew') : t('details.chips.add')} onKeyDown={enter} />
        <Popover className="popover select-popover">
          <ListBox className="select-list">
            {(option: { key: string; text: string }) => (
              <ListBoxItem id={option.key} className="select-item" textValue={option.text}>
                {option.text}
              </ListBoxItem>
            )}
          </ListBox>
        </Popover>
      </ComboBox>
    </div>
  );
}

function CustomInput({ field, value, onChange }: { field: CustomField; value: string | number | boolean | null; onChange(value: string | number | boolean | null): void }): React.JSX.Element {
  switch (field.kind) {
    case 'boolean':
      return <Tick label={field.name} isSelected={value === true} onChange={onChange} />;
    case 'choice':
      return <Picker label={field.name} value={typeof value === 'string' ? value : ''} options={[['', '—'], ...field.choices.map((choice): [string, string] => [choice, choice])]} onChange={(choice) => onChange(choice === '' ? null : choice)} />;
    default:
      return (
        <TextField className="field" value={value === null || value === undefined ? '' : String(value)} onChange={onChange} aria-label={field.name}>
          <Input className="text-input" type={field.kind === 'date' ? 'date' : 'text'} inputMode={field.kind === 'number' ? 'decimal' : undefined} placeholder={field.kind === 'url' ? 'https://' : ''} />
        </TextField>
      );
  }
}

/** Where the book is in the collection the side shows, for moving it there. */
export interface CollectionPlace {
  collection: number;
  name: string;
  /** The works just before and after it, if any. */
  before: number | null;
  after: number | null;
}

export interface InspectorProps {
  book: BookRow;
  taxonomy: Taxonomy;
  /** The collection the side shows, if the book is in it. */
  place: CollectionPlace | null;
  onSaved(saved?: WorkMetadata): void;
  onClose(): void;
}

export function Inspector({ book, taxonomy, place, onSaved, onClose }: InspectorProps): React.JSX.Element {
  const t = useT();
  const [work, setWork] = useState<WorkMetadata | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const exchange = useExchange({ kind: 'works', works: [book.workId] }, undefined, 'details');

  useEffect(() => {
    let current = true;
    setWork(null);
    setDraft(null);
    setError(null);
    api
      .workMetadata(book.workId)
      .then((loaded) => {
        if (current) {
          setWork(loaded);
        }
      })
      .catch((reason: unknown) => current && setError(reasonOf(reason)));
    return () => {
      current = false;
    };
  }, [book.workId]);

  // The draft starts from the work, and starts again only when the work is read again: a change
  // made on the side meanwhile (a new category, a tag renamed) leaves what is being typed alone.
  const original = useMemo(() => (work === null ? null : draftOf(work)), [work]);
  useEffect(() => setDraft(original === null ? null : structuredClone(original)), [original]);

  if (work === null || draft === null || original === null) {
    return (
      <aside className="inspector" aria-label={t('details.label')} aria-busy={error === null}>
        <p className="muted">{error ?? t('details.reading')}</p>
      </aside>
    );
  }
  const { edit, problems } = editOf(original, draft, taxonomy.fields);
  const changed = Object.keys(edit).length > 0;
  const set = <K extends keyof Draft>(key: K, value: Draft[K]): void => setDraft({ ...draft, [key]: value });
  const save = (): void => {
    if (problems.length > 0 || !changed) {
      return;
    }
    setSaving(true);
    setError(null);
    api
      .editWork(work.id, edit)
      .then((saved) => {
        setWork(saved);
        onSaved(saved);
      })
      .catch((reason: unknown) => setError(reasonOf(reason)))
      .finally(() => setSaving(false));
  };
  const categories = draft.categories.map((id) => ({ key: String(id), text: categoryPath(taxonomy.categories, id) }));
  const collections = draft.collections.map((id) => ({ key: String(id), text: taxonomy.collections.find((collection) => collection.id === id)?.name ?? String(id) }));

  return (
    <aside className="inspector" aria-label={t('details.label')} aria-busy={saving} data-work={work.id}>
      <header className="inspector-head">
        <h2>{t('details.title')}</h2>
        <Button className="button small" aria-label={t('details.close')} onPress={onClose}>
          ×
        </Button>
      </header>
      <ScrollFrame target=".inspector-body">
        <div className="inspector-body">
          <p className="muted file-line" title={book.path}>
            {work.formats.map((format) => format.toUpperCase()).join(', ')} · {book.path.split(/[\\/]/).at(-1)}
          </p>

          <TextField className="field" value={draft.title} onChange={(title) => set('title', title)}>
            <Label>
              {t('details.field.title')} <Origin work={work} field="title" />
            </Label>
            <Input className="text-input" data-field="title" />
          </TextField>

          <div className="field" role="group" aria-label={t('details.creators')}>
            <span className="field-label">
              {t('details.creators')} <Origin work={work} field="creators" />
            </span>
            {draft.creators.map((credit, index) => (
              <div key={index} className="row">
                <TextField
                  className="grow"
                  value={credit.name}
                  aria-label={t('details.creatorName', { number: index + 1 })}
                  onChange={(name) => set('creators', draft.creators.map((old, at) => (at === index ? { ...old, name } : old)))}
                >
                  <Input className="text-input" />
                </TextField>
                <Picker label={t('details.creatorRole', { number: index + 1 })} value={credit.role} options={roles()} onChange={(role) => set('creators', draft.creators.map((old, at) => (at === index ? { ...old, role } : old)))} />
                <Button
                  className="button small"
                  aria-label={t('details.creatorUp', { name: credit.name })}
                  isDisabled={index === 0}
                  onPress={() => {
                    const next = [...draft.creators];
                    [next[index - 1], next[index]] = [next[index], next[index - 1]];
                    set('creators', next);
                  }}
                >
                  ↑
                </Button>
                <Button className="button small" aria-label={t('details.removeChip', { name: credit.name })} onPress={() => set('creators', draft.creators.filter((_, at) => at !== index))}>
                  ×
                </Button>
              </div>
            ))}
            <Button className="button small" onPress={() => set('creators', [...draft.creators, { name: '', role: 'author' }])}>
              {t('details.addCreator')}
            </Button>
          </div>

          <div className="row">
            <TextField className="field grow" value={draft.series} onChange={(series) => set('series', series)}>
              <Label>
                {t('details.series')} <Origin work={work} field="series" />
              </Label>
              <Input className="text-input" data-field="series" />
            </TextField>
            <TextField className="field narrow" value={draft.seriesPosition} onChange={(position) => set('seriesPosition', position)}>
              <Label>{t('details.seriesPosition')}</Label>
              <Input className="text-input" inputMode="decimal" data-field="series-position" />
            </TextField>
          </div>

          <div className="row">
            <TextField className="field narrow" value={draft.year} onChange={(year) => set('year', year)}>
              <Label>
                {t('details.year')} <Origin work={work} field="year" />
              </Label>
              <Input className="text-input" inputMode="numeric" data-field="year" />
            </TextField>
            <TextField className="field grow" value={draft.language} onChange={(language) => set('language', language)}>
              <Label>
                {t('details.language')} <Origin work={work} field="language" />
              </Label>
              <Input className="text-input" placeholder="pt-BR" data-field="language" />
            </TextField>
          </div>

          <div className="field" role="group" aria-label={t('details.identifiers')}>
            <span className="field-label">
              {t('details.identifiers')} <Origin work={work} field="identifiers" />
            </span>
            {draft.identifiers.map((identifier, index) => (
              <div key={index} className="row">
                <Picker label={t('details.scheme', { number: index + 1 })} value={identifier.scheme} options={schemes()} onChange={(scheme) => set('identifiers', draft.identifiers.map((old, at) => (at === index ? { ...old, scheme } : old)))} />
                <TextField
                  className="grow"
                  value={identifier.value}
                  aria-label={t('details.identifier', { number: index + 1 })}
                  onChange={(value) => set('identifiers', draft.identifiers.map((old, at) => (at === index ? { ...old, value } : old)))}
                >
                  <Input className="text-input" />
                </TextField>
                <Button className="button small" aria-label={t('details.removeIdentifier')} onPress={() => set('identifiers', draft.identifiers.filter((_, at) => at !== index))}>
                  ×
                </Button>
              </div>
            ))}
            <Button className="button small" onPress={() => set('identifiers', [...draft.identifiers, { scheme: 'isbn', value: '' }])}>
              {t('details.addIdentifier')}
            </Button>
          </div>

          <div className="row">
            <div className="field" role="group" aria-label={t('details.rating')}>
              <span className="field-label">{t('details.rating')}</span>
              <div className="stars">
                {[1, 2, 3, 4, 5].map((stars) => (
                  <ToggleButton
                    key={stars}
                    className="star"
                    isSelected={draft.rating !== null && stars <= draft.rating}
                    aria-label={tn('details.stars', stars)}
                    onChange={() => set('rating', draft.rating === stars ? null : stars)}
                  >
                    ★
                  </ToggleButton>
                ))}
              </div>
            </div>
            <div className="field" role="group" aria-label={t('details.statusGroup')}>
              <span className="field-label">{t('details.status')}</span>
              <Picker label={t('details.statusGroup')} value={draft.status} options={statuses()} onChange={(status) => set('status', status)} />
            </div>
          </div>

          <TextField className="field" value={draft.description} onChange={(description) => set('description', description)}>
            <Label>
              {t('details.description')} <Origin work={work} field="description" />
            </Label>
            <TextArea className="text-input description" rows={4} data-field="description" />
          </TextField>

          <div className="field" role="group" aria-label={t('details.categories')}>
            <span className="field-label">{t('details.categories')}</span>
            <Chips
              label={t('details.categories')}
              chosen={categories}
              options={taxonomy.categories.map((category) => ({ key: String(category.id), text: categoryPath(taxonomy.categories, category.id) }))}
              onChange={(keys) => set('categories', keys.map(Number))}
            />
          </div>

          <div className="field" role="group" aria-label={t('details.tags')}>
            <span className="field-label">{t('details.tags')}</span>
            <Chips
              label={t('details.tags')}
              chosen={draft.tags.map((name) => ({ key: name, text: name }))}
              options={taxonomy.tags.map((tag) => ({ key: tag.name, text: tag.name }))}
              allowsNew
              onChange={(names) => set('tags', names)}
            />
          </div>

          {place !== null ? (
            <div className="field" role="group" aria-label={t('details.orderIn', { name: place.name })}>
              <span className="field-label">{t('details.orderInShown', { name: place.name })}</span>
              <div className="row">
                <Button
                  className="button small"
                  isDisabled={place.before === null}
                  onPress={() => {
                    if (place.before !== null) {
                      api.addToCollection(place.collection, [work.id], { at: 'before', id: place.before }).then(() => onSaved()).catch((reason: unknown) => setError(reasonOf(reason)));
                    }
                  }}
                >
                  {t('details.up')}
                </Button>
                <Button
                  className="button small"
                  isDisabled={place.after === null}
                  onPress={() => {
                    if (place.after !== null) {
                      api.addToCollection(place.collection, [work.id], { at: 'after', id: place.after }).then(() => onSaved()).catch((reason: unknown) => setError(reasonOf(reason)));
                    }
                  }}
                >
                  {t('details.down')}
                </Button>
              </div>
            </div>
          ) : null}

          <div className="field" role="group" aria-label={t('details.collections')}>
            <span className="field-label">{t('details.collections')}</span>
            <Chips
              label={t('details.collections')}
              chosen={collections}
              options={taxonomy.collections.map((collection) => ({ key: String(collection.id), text: collection.name }))}
              onChange={(keys) => set('collections', keys.map(Number))}
            />
          </div>

          {taxonomy.fields.length > 0 ? (
            <div className="field" role="group" aria-label={t('details.custom')}>
              <span className="field-label">{t('details.custom')}</span>
              {taxonomy.fields.map((field) => (
                <div key={field.id} className="custom-row">
                  {field.kind === 'boolean' ? null : <span className="custom-name">{field.name}</span>}
                  <CustomInput field={field} value={draft.custom[field.id] ?? null} onChange={(value) => set('custom', { ...draft.custom, [field.id]: value })} />
                </div>
              ))}
            </div>
          ) : null}
          <ExchangeSection
            heading={t('details.annotations')}
            controls={exchange}
            id="work-exchange"
            className="inspector-exchange"
            idle={t('details.annotations.idle')}
          />
        </div>
      </ScrollFrame>
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
          <Button className="button" isDisabled={!changed || saving} onPress={() => setDraft(structuredClone(original))}>
            {t('details.discard')}
          </Button>
          <Button id="save-work" className="button primary" isDisabled={!changed || saving || problems.length > 0} onPress={save}>
            {t('common.save')}
          </Button>
        </div>
      </footer>
    </aside>
  );
}
