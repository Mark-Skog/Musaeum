/**
 * The library's dialogs: naming something (a category, a tag, a collection), moving a category,
 * confirming a removal, writing a smart collection's rule, and defining custom fields. Each says
 * what went wrong in place, and closes only when what it asked for was done.
 */

import { type FormEvent, useEffect, useMemo, useState } from 'react';
import {
  Button,
  Checkbox,
  Dialog,
  Heading,
  Input,
  Label,
  ListBox,
  ListBoxItem,
  Modal,
  ModalOverlay,
  Popover,
  Radio,
  RadioGroup,
  Select,
  SelectValue,
  TextArea,
  TextField,
} from 'react-aria-components';

import { type MessageKey, t, tn, useLocale, useT } from '../i18n';
import { ScrollFrame } from '../OverlayScrollbar';
import { api, type Category, type CustomField, type FieldKind, type FolderEntry, type RootSummary, type SmartCollection, type SmartRule, type SmartRules, type Tag, type Taxonomy } from '../api';
import { shortName } from './sidebar-model';
import { categoryPath, defaultRule, fieldKinds, FORMATS, moveTargets, ruleFields, ruleOps, ruleProblem, statuses } from './taxonomy-model';

/** What the host said when it refused, without the noise of the transport. */
export function reasonOf(error: unknown): string {
  return String(error).replace(/^Error:\s*/, '');
}

export function Frame({
  title,
  onClose,
  onSubmit,
  submitLabel,
  error,
  busy,
  danger,
  wide,
  children,
}: {
  title: string;
  onClose(): void;
  onSubmit(): void;
  submitLabel: string;
  error: string | null;
  busy?: boolean;
  danger?: boolean;
  wide?: boolean;
  children: React.ReactNode;
}): React.JSX.Element {
  const t = useT();
  const submit = (event: FormEvent): void => {
    event.preventDefault();
    if (!busy) {
      onSubmit();
    }
  };
  return (
    <ModalOverlay isOpen isDismissable onOpenChange={(open) => !open && onClose()} className="modal-overlay">
      <Modal className={wide ? 'modal library-dialog wide' : 'modal library-dialog'}>
        <Dialog aria-label={title} className="library-dialog-body">
          <form onSubmit={submit} className="library-form">
            <Heading slot="title" className="dialog-title">
              {title}
            </Heading>
            {children}
            {error !== null ? (
              <p className="form-error" role="alert">
                {error}
              </p>
            ) : null}
            <div className="modal-actions">
              <Button className="button" onPress={onClose}>
                {t('common.cancel')}
              </Button>
              <Button type="submit" className={danger ? 'button danger' : 'button primary'} isDisabled={busy}>
                {submitLabel}
              </Button>
            </div>
          </form>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}

/** Runs what a dialog asks for, and keeps it open with the host's reason if it fails. */
function useSubmit(onDone: () => void): { error: string | null; busy: boolean; run(action: () => Promise<unknown>): void; setError(error: string | null): void } {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const run = (action: () => Promise<unknown>): void => {
    setBusy(true);
    setError(null);
    action()
      .then(onDone)
      .catch((reason: unknown) => setError(reasonOf(reason)))
      .finally(() => setBusy(false));
  };
  return { error, busy, run, setError };
}

export function NameDialog({
  title,
  label,
  initial,
  submitLabel,
  onSubmit,
  onClose,
}: {
  title: string;
  label: string;
  initial: string;
  submitLabel: string;
  onSubmit(name: string): Promise<unknown>;
  onClose(): void;
}): React.JSX.Element {
  const [name, setName] = useState(initial);
  const { error, busy, run, setError } = useSubmit(onClose);
  return (
    <Frame
      title={title}
      onClose={onClose}
      submitLabel={submitLabel}
      error={error}
      busy={busy}
      onSubmit={() => (name.trim() === '' ? setError(t('dialogs.giveName')) : run(() => onSubmit(name.trim())))}
    >
      <TextField className="field" value={name} onChange={setName} autoFocus aria-label={label}>
        <Label>{label}</Label>
        <Input className="text-input" />
      </TextField>
    </Frame>
  );
}

export function ConfirmDialog({
  title,
  message,
  confirmLabel,
  onConfirm,
  onClose,
}: {
  title: string;
  message: string;
  confirmLabel: string;
  onConfirm(): Promise<unknown>;
  onClose(): void;
}): React.JSX.Element {
  const { error, busy, run } = useSubmit(onClose);
  return (
    <Frame title={title} onClose={onClose} submitLabel={confirmLabel} error={error} busy={busy} danger onSubmit={() => run(onConfirm)}>
      <p className="dialog-text">{message}</p>
    </Frame>
  );
}

/** Where a category goes: under another, or to the top. */
export function MoveCategoryDialog({ categories, id, onClose, onMoved }: { categories: Category[]; id: number; onClose(): void; onMoved(): void }): React.JSX.Element {
  const t = useT();
  const self = categories.find((category) => category.id === id);
  const targets = moveTargets(categories, id);
  const [parent, setParent] = useState<string>(self?.parentId === null || self === undefined ? 'top' : String(self.parentId));
  const { error, busy, run } = useSubmit(() => {
    onMoved();
    onClose();
  });
  return (
    <Frame
      title={t('dialogs.move.title', { name: self?.name ?? '' })}
      onClose={onClose}
      submitLabel={t('dialogs.move.submit')}
      error={error}
      busy={busy}
      onSubmit={() => run(() => api.moveCategory(id, parent === 'top' ? null : Number(parent), { at: 'last' }))}
    >
      <Select className="field" selectedKey={parent} onSelectionChange={(key) => setParent(String(key))} aria-label={t('dialogs.move.into')}>
        <Label>{t('dialogs.move.into')}</Label>
        <Button className="button select-button">
          <SelectValue />
          <span aria-hidden="true">▾</span>
        </Button>
        <Popover className="popover select-popover">
          <ListBox className="select-list">
            <ListBoxItem id="top" className="select-item" textValue={t('dialogs.move.top')}>
              {t('dialogs.move.topShown')}
            </ListBoxItem>
            {targets.map((target) => (
              <ListBoxItem key={target.id} id={String(target.id)} className="select-item" textValue={categoryPath(categories, target.id)}>
                {categoryPath(categories, target.id)}
              </ListBoxItem>
            ))}
          </ListBox>
        </Popover>
      </Select>
      <p className="dialog-text muted">{t('dialogs.move.hint')}</p>
    </Frame>
  );
}

/** A tag's colors, with the key of each one's name. */
export const TAG_COLORS: [string, MessageKey][] = [
  ['yellow', 'annot.color.yellow'],
  ['green', 'annot.color.green'],
  ['blue', 'annot.color.blue'],
  ['pink', 'annot.color.pink'],
  ['purple', 'annot.color.purple'],
  ['orange', 'dialogs.color.orange'],
  ['gray', 'dialogs.color.gray'],
];

export function TagDialog({ tag, onClose, onSaved }: { tag: Tag | null; onClose(): void; onSaved(): void }): React.JSX.Element {
  const t = useT();
  const [name, setName] = useState(tag?.name ?? '');
  const [color, setColor] = useState<string>(tag?.color ?? 'none');
  const { error, busy, run, setError } = useSubmit(() => {
    onSaved();
    onClose();
  });
  const save = (): void => {
    if (name.trim() === '') {
      setError(t('dialogs.giveName'));
      return;
    }
    const chosen = color === 'none' ? null : color;
    run(() => (tag === null ? api.createTag(name.trim(), chosen) : api.updateTag(tag.id, name.trim() === tag.name ? null : name.trim(), chosen)));
  };
  return (
    <Frame title={tag === null ? t('dialogs.tag.new') : t('dialogs.tag.edit')} onClose={onClose} submitLabel={tag === null ? t('dialogs.create') : t('common.save')} error={error} busy={busy} onSubmit={save}>
      <TextField className="field" value={name} onChange={setName} autoFocus>
        <Label>{t('dialogs.name')}</Label>
        <Input className="text-input" />
      </TextField>
      <RadioGroup className="field" value={color} onChange={setColor} orientation="horizontal">
        <Label>{t('dialogs.color')}</Label>
        <div className="color-choices">
          <Radio value="none" className="color-choice none" aria-label={t('dialogs.color.none')} />
          {TAG_COLORS.map(([value, label]) => (
            <Radio key={value} value={value} className={`color-choice tag-${value}`} aria-label={t(label)} />
          ))}
        </div>
      </RadioGroup>
    </Frame>
  );
}

export function CollectionDialog({ collection, onClose, onSaved }: { collection: { id: number; name: string; description: string } | null; onClose(): void; onSaved(): void }): React.JSX.Element {
  const t = useT();
  const [name, setName] = useState(collection?.name ?? '');
  const [description, setDescription] = useState(collection?.description ?? '');
  const { error, busy, run, setError } = useSubmit(() => {
    onSaved();
    onClose();
  });
  const save = (): void => {
    if (name.trim() === '') {
      setError(t('dialogs.giveName'));
      return;
    }
    run(() => (collection === null ? api.createCollection(name.trim(), description, { at: 'last' }) : api.updateCollection(collection.id, name.trim(), description)));
  };
  return (
    <Frame title={collection === null ? t('dialogs.collection.new') : t('dialogs.collection.edit')} onClose={onClose} submitLabel={collection === null ? t('dialogs.create') : t('common.save')} error={error} busy={busy} onSubmit={save}>
      <TextField className="field" value={name} onChange={setName} autoFocus>
        <Label>{t('dialogs.name')}</Label>
        <Input className="text-input" />
      </TextField>
      <TextField className="field" value={description} onChange={setDescription}>
        <Label>{t('dialogs.description')}</Label>
        <TextArea className="text-input" rows={3} />
      </TextField>
    </Frame>
  );
}

/** One choice of a select, as a list of `[id, label]`. */
function Choice({ label, value, options, onChange }: { label: string; value: string; options: [string, string][]; onChange(value: string): void }): React.JSX.Element {
  return (
    <Select className="rule-part" selectedKey={value} onSelectionChange={(key) => onChange(String(key))} aria-label={label} data-picker={label}>
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

function folderLabel(folders: FolderEntry[], id: number): string {
  const byId = new Map(folders.map((folder) => [folder.id, folder]));
  const names: string[] = [];
  for (let at = byId.get(id); at !== undefined; at = at.parentId === null ? undefined : byId.get(at.parentId)) {
    names.unshift(at.parentId === null ? (at.name.split(/[\\/]/).filter(Boolean).at(-1) ?? at.name) : at.name);
  }
  return names.join(' › ');
}

/** A smart collection: its name, whether all rules or any must match, and the rules. */
export function SmartDialog({
  smart,
  taxonomy,
  folders,
  onClose,
  onSaved,
}: {
  smart: SmartCollection | null;
  taxonomy: Taxonomy;
  folders: FolderEntry[];
  onClose(): void;
  onSaved(): void;
}): React.JSX.Element {
  const t = useT();
  const locale = useLocale();
  // The fields' names are in the language shown.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const fields = useMemo(() => ruleFields(taxonomy.fields), [taxonomy.fields, locale]);
  const [name, setName] = useState(smart?.name ?? '');
  const [rules, setRules] = useState<SmartRules>(smart?.rules ?? { match: 'all', rules: [defaultRule(fields[0])] });
  const [count, setCount] = useState<number | null>(null);
  const { error, busy, run, setError } = useSubmit(() => {
    onSaved();
    onClose();
  });
  const problems = rules.rules.map((rule) => ruleProblem(rule, fields));
  const complete = rules.rules.length > 0 && problems.every((problem) => problem === null);
  const key = JSON.stringify(rules);
  useEffect(() => {
    if (!complete) {
      setCount(null);
      return;
    }
    let current = true;
    const timer = window.setTimeout(() => {
      api
        .previewSmart(JSON.parse(key) as SmartRules)
        .then((found) => current && setCount(found))
        .catch(() => current && setCount(null));
    }, 250);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [key, complete]);
  const change = (index: number, rule: SmartRule): void => setRules({ ...rules, rules: rules.rules.map((old, at) => (at === index ? rule : old)) });
  const setOf = (set: string | undefined): [string, string][] => {
    switch (set) {
      case 'tag':
        return taxonomy.tags.map((tag) => [String(tag.id), tag.name]);
      case 'category':
        return taxonomy.categories.map((category) => [String(category.id), categoryPath(taxonomy.categories, category.id)]);
      case 'collection':
        return taxonomy.collections.map((collection) => [String(collection.id), collection.name]);
      default:
        return folders.map((folder) => [String(folder.id), folderLabel(folders, folder.id)]);
    }
  };
  const save = (): void => {
    if (name.trim() === '') {
      setError(t('dialogs.giveName'));
      return;
    }
    if (!complete) {
      setError(problems.find((problem) => problem !== null) ?? t('dialogs.smart.addRule'));
      return;
    }
    run(() => (smart === null ? api.createSmart(name.trim(), rules, { at: 'last' }) : api.updateSmart(smart.id, name.trim(), rules)));
  };
  return (
    <Frame title={smart === null ? t('dialogs.smart.new') : t('dialogs.smart.edit')} onClose={onClose} submitLabel={smart === null ? t('dialogs.create') : t('common.save')} error={error} busy={busy} wide onSubmit={save}>
      <TextField className="field" value={name} onChange={setName} autoFocus>
        <Label>{t('dialogs.name')}</Label>
        <Input className="text-input" />
      </TextField>
      <RadioGroup className="field" value={rules.match} onChange={(value) => setRules({ ...rules, match: value as 'all' | 'any' })} orientation="horizontal">
        <Label>{t('dialogs.smart.match')}</Label>
        <div className="choices">
          <Radio value="all" className="choice">
            {t('dialogs.smart.all')}
          </Radio>
          <Radio value="any" className="choice">
            {t('dialogs.smart.any')}
          </Radio>
        </div>
      </RadioGroup>
      <ol className="rules" aria-label={t('dialogs.smart.rules')}>
        {rules.rules.map((rule, index) => {
          const field = fields.find((candidate) => candidate.id === rule.field) ?? fields[0];
          const ops = ruleOps(field.kind);
          const op = ops.find((candidate) => candidate.id === rule.op) ?? ops[0];
          const number = (value: unknown): string => (typeof value === 'number' ? String(value) : '');
          const toNumber = (text: string): number | string => (text.trim() === '' || !Number.isFinite(Number(text.replace(',', '.'))) ? text : Number(text.replace(',', '.')));
          let input: React.ReactNode = null;
          if (op.takes === 'two') {
            const pair = Array.isArray(rule.value) ? (rule.value as unknown[]) : [null, null];
            input = (
              <span className="rule-part pair">
                <Input className="text-input small" aria-label={t('dialogs.smart.from')} defaultValue={number(pair[0])} onChange={(event) => change(index, { ...rule, value: [toNumber(event.target.value), pair[1]] })} />
                <span>{t('dialogs.smart.and')}</span>
                <Input className="text-input small" aria-label={t('dialogs.smart.to')} defaultValue={number(pair[1])} onChange={(event) => change(index, { ...rule, value: [pair[0], toNumber(event.target.value)] })} />
              </span>
            );
          } else if (op.takes === 'one') {
            if (field.kind === 'set') {
              input = <Choice label={t('dialogs.smart.value')} value={rule.value === null || rule.value === '' ? '' : String(rule.value)} options={setOf(field.set)} onChange={(value) => change(index, { ...rule, value: Number(value) })} />;
            } else if (field.kind === 'status') {
              input = <Choice label={t('dialogs.smart.value')} value={String(rule.value)} options={statuses()} onChange={(value) => change(index, { ...rule, value })} />;
            } else if (field.kind === 'format') {
              input = <Choice label={t('dialogs.smart.value')} value={String(rule.value)} options={FORMATS.map((format) => [format, format.toUpperCase()])} onChange={(value) => change(index, { ...rule, value })} />;
            } else if (field.choices !== undefined && (op.id === 'is' || op.id === 'is_not')) {
              input = <Choice label={t('dialogs.smart.value')} value={String(rule.value)} options={field.choices.map((choice) => [choice, choice])} onChange={(value) => change(index, { ...rule, value })} />;
            } else {
              const numeric = field.kind === 'number' || op.id === 'in_last_days';
              input = (
                <Input
                  className="text-input rule-part"
                  aria-label={t('dialogs.smart.value')}
                  placeholder={op.id === 'before' || op.id === 'after' ? t('dialogs.datePlaceholder') : ''}
                  defaultValue={typeof rule.value === 'string' || typeof rule.value === 'number' ? String(rule.value) : ''}
                  onChange={(event) => change(index, { ...rule, value: numeric ? toNumber(event.target.value) : event.target.value })}
                />
              );
            }
          }
          return (
            <li key={index} className="rule">
              <Choice label={t('dialogs.smart.field')} value={field.id} options={fields.map((candidate) => [candidate.id, candidate.label])} onChange={(value) => change(index, defaultRule(fields.find((candidate) => candidate.id === value) ?? fields[0]))} />
              <Choice label={t('dialogs.smart.condition')} value={op.id} options={ops.map((candidate) => [candidate.id, candidate.label])} onChange={(value) => {
                const next = ops.find((candidate) => candidate.id === value) ?? ops[0];
                change(index, { ...rule, op: value, value: next.takes === op.takes ? rule.value : next.takes === 'two' ? [0, 0] : next.takes === 'none' ? null : '' });
              }} />
              <span key={`${rule.field}-${rule.op}`} className="rule-value">
                {input}
              </span>
              <Button className="button small" aria-label={t('dialogs.smart.removeRule')} onPress={() => setRules({ ...rules, rules: rules.rules.filter((_, at) => at !== index) })}>
                ×
              </Button>
              {problems[index] !== null ? <span className="rule-problem">{problems[index]}</span> : null}
            </li>
          );
        })}
      </ol>
      <div className="rules-footer">
        <Button className="button" onPress={() => setRules({ ...rules, rules: [...rules.rules, defaultRule(fields[0])] })} isDisabled={rules.rules.length >= 32}>
          {t('dialogs.smart.addRuleButton')}
        </Button>
        <span className="muted" aria-live="polite">
          {count === null ? '' : tn('dialogs.smart.count', count)}
        </span>
      </div>
    </Frame>
  );
}

/** The custom fields: their names, kinds and choices; a field is added, renamed or removed. */
export function FieldsDialog({ fields, onClose, onChanged }: { fields: CustomField[]; onClose(): void; onChanged(): void }): React.JSX.Element {
  const t = useT();
  const [name, setName] = useState('');
  const [kind, setKind] = useState<FieldKind>('text');
  const [choices, setChoices] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [removing, setRemoving] = useState<CustomField | null>(null);
  const add = (): void => {
    setError(null);
    const list = choices
      .split('\n')
      .map((choice) => choice.trim())
      .filter((choice) => choice !== '');
    if (name.trim() === '') {
      setError(t('dialogs.fields.giveName'));
      return;
    }
    if (kind === 'choice' && list.length === 0) {
      setError(t('dialogs.fields.giveChoices'));
      return;
    }
    api
      .createField(name.trim(), kind, kind === 'choice' ? list : [])
      .then(() => {
        setName('');
        setChoices('');
        onChanged();
      })
      .catch((reason: unknown) => setError(reasonOf(reason)));
  };
  return (
    <>
      <ModalOverlay isOpen isDismissable onOpenChange={(open) => !open && onClose()} className="modal-overlay">
        <Modal className="modal library-dialog wide">
          <Dialog aria-label={t('dialogs.fields.title')} className="library-dialog-body">
            <div className="library-form">
              <Heading slot="title" className="dialog-title">
                {t('dialogs.fields.title')}
              </Heading>
              {fields.length === 0 ? (
                <p className="dialog-text muted">{t('dialogs.fields.none')}</p>
              ) : (
                <ul className="field-list" aria-label={t('dialogs.fields.list')}>
                  {fields.map((field) => (
                    <li key={field.id} className="field-row">
                      <span className="field-name">{field.name}</span>
                      <span className="muted">
                        {fieldKinds().find(([id]) => id === field.kind)?.[1]}
                        {field.kind === 'choice' ? `: ${field.choices.join(', ')}` : ''}
                      </span>
                      <Button className="button small" aria-label={t('dialogs.fields.removeNamed', { name: field.name })} onPress={() => setRemoving(field)}>
                        {t('dialogs.fields.remove')}
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
              <form
                className="field-add"
                onSubmit={(event) => {
                  event.preventDefault();
                  add();
                }}
              >
                <TextField className="field" value={name} onChange={setName}>
                  <Label>{t('dialogs.fields.new')}</Label>
                  <Input className="text-input" />
                </TextField>
                <Select className="field" selectedKey={kind} onSelectionChange={(key) => setKind(key as FieldKind)}>
                  <Label>{t('dialogs.fields.kind')}</Label>
                  <Button className="button select-button">
                    <SelectValue />
                    <span aria-hidden="true">▾</span>
                  </Button>
                  <Popover className="popover select-popover">
                    <ListBox className="select-list">
                      {fieldKinds().map(([id, label]) => (
                        <ListBoxItem key={id} id={id} className="select-item">
                          {label}
                        </ListBoxItem>
                      ))}
                    </ListBox>
                  </Popover>
                </Select>
                {kind === 'choice' ? (
                  <TextField className="field" value={choices} onChange={setChoices}>
                    <Label>{t('dialogs.fields.choices')}</Label>
                    <TextArea className="text-input" rows={3} />
                  </TextField>
                ) : null}
                <Button type="submit" className="button">
                  {t('dialogs.fields.add')}
                </Button>
              </form>
              {error !== null ? (
                <p className="form-error" role="alert">
                  {error}
                </p>
              ) : null}
              <div className="modal-actions">
                <Button className="button primary" onPress={onClose}>
                  {t('common.close')}
                </Button>
              </div>
            </div>
          </Dialog>
        </Modal>
      </ModalOverlay>
      {removing !== null ? (
        <ConfirmDialog
          title={t('dialogs.fields.confirmTitle', { name: removing.name })}
          message={t('dialogs.fields.confirmMessage')}
          confirmLabel={t('dialogs.fields.remove')}
          onConfirm={() => api.deleteField(removing.id).then(onChanged)}
          onClose={() => setRemoving(null)}
        />
      ) : null}
    </>
  );
}

/** A checkbox as React Aria draws it, for the editor. */
export function Tick({ label, isSelected, onChange }: { label: string; isSelected: boolean; onChange(value: boolean): void }): React.JSX.Element {
  return (
    <Checkbox className="tick" isSelected={isSelected} onChange={onChange}>
      <span className="tick-box" aria-hidden="true">
        {isSelected ? '✓' : ''}
      </span>
      {label}
    </Checkbox>
  );
}

/** Monitored library roots: lets the user inspect, add, or stop following folders. */
export function FoldersDialog({
  roots,
  onClose,
  onAddFolders,
  onRemoveRoot,
}: {
  roots: RootSummary[];
  onClose(): void;
  onAddFolders(): Promise<void>;
  onRemoveRoot(id: number): Promise<void>;
}): React.JSX.Element {
  const t = useT();
  const [removing, setRemoving] = useState<RootSummary | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const add = (): void => {
    setError(null);
    setBusy(true);
    onAddFolders()
      .catch((reason: unknown) => setError(reasonOf(reason)))
      .finally(() => setBusy(false));
  };

  return (
    <>
      <ModalOverlay isOpen isDismissable onOpenChange={(open) => !open && onClose()} className="modal-overlay">
        <Modal className="modal library-dialog wide">
          <Dialog aria-label={t('dialogs.folders.title')} className="library-dialog-body">
            <div className="library-form">
              <Heading slot="title" className="dialog-title">
                {t('dialogs.folders.title')}
              </Heading>
              <p className="dialog-text muted">{t('dialogs.folders.intro')}</p>
              {roots.length === 0 ? (
                <p className="dialog-text muted">{t('dialogs.folders.none')}</p>
              ) : (
                <ScrollFrame className="shrink" target=".roots-list">
                  <ul className="roots-list" aria-label={t('dialogs.folders.list')}>
                    {roots.map((root) => (
                      <li key={root.id} className="root-row">
                        <div className="root-meta">
                          <span className="root-name">{shortName(root.path)}</span>
                          <span className="root-path muted" title={root.path}>
                            {root.path}
                          </span>
                        </div>
                        <Button className="button small" aria-label={t('dialogs.folders.removeNamed', { name: shortName(root.path) })} onPress={() => setRemoving(root)}>
                          {t('dialogs.folders.remove')}
                        </Button>
                      </li>
                    ))}
                  </ul>
                </ScrollFrame>
              )}
              {error !== null ? (
                <p className="form-error" role="alert">
                  {error}
                </p>
              ) : null}
              <div className="modal-actions">
                <Button className="button" onPress={add} isDisabled={busy}>
                  {t('dialogs.folders.add')}
                </Button>
                <Button className="button primary" onPress={onClose}>
                  {t('common.close')}
                </Button>
              </div>
            </div>
          </Dialog>
        </Modal>
      </ModalOverlay>
      {removing !== null ? (
        <ConfirmDialog
          title={t('dialogs.folders.confirmTitle', { name: shortName(removing.path) })}
          message={t('dialogs.folders.confirmMessage')}
          confirmLabel={t('dialogs.folders.remove')}
          onConfirm={() => onRemoveRoot(removing.id)}
          onClose={() => setRemoving(null)}
        />
      ) : null}
    </>
  );
}
