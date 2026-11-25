/**
 * The Preferences: the interface's language, its colors and motion, the name annotations are
 * signed with, the OCR's languages, and what the library's cache holds. Each choice is kept and
 * applied as it is made; the dialog says what it did, or why it could not, in a live region.
 */

import { useCallback, useEffect, useId, useRef, useState } from 'react';
import {
  Button,
  Checkbox,
  CheckboxGroup,
  Dialog,
  Heading,
  Input,
  Label,
  Modal,
  ModalOverlay,
  Radio,
  RadioGroup,
  Text,
  TextField,
} from 'react-aria-components';

import { api, type CacheReport, type MotionChoice, type OcrModels, type ThemeChoice } from '../api';
import { formatBytes, LOCALES, languageName, type Locale, tn, useT } from '../i18n';
import { focusIsLost, useFocusReturn } from '../focus';
import { ScrollFrame } from '../OverlayScrollbar';
import { reasonOf } from '../library/dialogs';
import { clampCoverWidth, COVER_DEFAULT_WIDTH, COVER_MAX_WIDTH, COVER_MIN_WIDTH } from '../library/sidebar-model';
import { systemReducesMotion, updatePreferences, usePreferences } from './store';

/** Each language by its own name, the same in every language of the interface. */
const OWN_NAMES: Record<Locale, string> = { 'pt-BR': 'Português (Brasil)', en: 'English' };

export interface PreferencesDialogProps {
  onClose(): void;
}

export function PreferencesDialog({ onClose }: PreferencesDialogProps): React.JSX.Element {
  const t = useT();
  const view = usePreferences();
  const [status, setStatus] = useState<{ text: string; failed: boolean } | null>(null);
  const [author, setAuthor] = useState(view?.preferences.annotationAuthor ?? '');
  const [ocr, setOcr] = useState<OcrModels | null>(null);
  const [cache, setCache] = useState<CacheReport | null | undefined>(undefined);
  const [confirming, setConfirming] = useState(false);
  // The confirmation goes away with the focus in it, and a saved name turns its button off: the focus
  // comes back to the button that asks again, or to the name, so that it stays in the dialog (and
  // Escape still closes it).
  useFocusReturn(confirming, () => document.getElementById('prefs-cache-clear')?.focus());
  const authorInput = useRef<HTMLInputElement>(null);
  const [clearing, setClearing] = useState(false);
  const titleId = useId();

  useEffect(() => {
    void api.ocrModels().then(setOcr, () => setOcr(null));
    void api.cacheReport().then(setCache, () => setCache(null));
  }, []);

  const save = useCallback(
    async (patch: Parameters<typeof updatePreferences>[0], done?: string) => {
      try {
        await updatePreferences(patch);
        setStatus(done === undefined ? null : { text: done, failed: false });
        if (patch.ocrLanguages !== undefined) {
          setOcr(await api.ocrModels());
        }
      } catch (reason: unknown) {
        setStatus({ text: t('prefs.failed', { reason: reasonOf(reason) }), failed: true });
      }
    },
    [t],
  );

  const clearCache = async (): Promise<void> => {
    setClearing(true);
    setStatus({ text: t('prefs.cache.clearing'), failed: false });
    try {
      const after = await api.cacheClearLines();
      setCache(after);
      setStatus({ text: t('prefs.cache.cleared', { size: formatBytes(after?.totalBytes ?? 0) }), failed: false });
    } catch (reason: unknown) {
      setStatus({ text: t('prefs.failed', { reason: reasonOf(reason) }), failed: true });
    } finally {
      setClearing(false);
      setConfirming(false);
    }
  };

  const preferences = view?.preferences;
  const verified = ocr?.models.filter((model) => model.verified) ?? [];
  const unusable = ocr?.models.filter((model) => !model.verified) ?? [];
  const fetchable = ocr?.fetchable.filter((model) => !model.shipped) ?? [];
  const name = (bcp47: string | null, code: string): string => (bcp47 === null ? code : `${languageName(bcp47)} (${code})`);

  return (
    <ModalOverlay isOpen isDismissable onOpenChange={(open) => !open && onClose()} className="modal-overlay">
      <Modal className="modal prefs-modal">
        <Dialog aria-labelledby={titleId} className="prefs-dialog" data-surface="preferences">
          <header className="prefs-header">
            <Heading slot="title" id={titleId} className="dialog-title">
              {t('prefs.title')}
            </Heading>
            <Button className="button" onPress={onClose} id="prefs-close">
              {t('prefs.close')}
            </Button>
          </header>

          {view?.problem !== null && view?.problem !== undefined ? (
            <p className="form-error" role="alert">
              {t('prefs.problem', { problem: view.problem })}
            </p>
          ) : null}

          <ScrollFrame target=".prefs-body">
            <div className="prefs-body">
            <section className="prefs-section" aria-labelledby={`${titleId}-language`}>
              <h3 id={`${titleId}-language`} className="prefs-heading">
                {t('prefs.language.heading')}
              </h3>
              <RadioGroup
                className="prefs-choices"
                value={preferences?.language ?? 'pt-BR'}
                onChange={(language) => void save({ language })}
                id="prefs-language"
              >
                <Label className="field-label">{t('prefs.language.label')}</Label>
                {LOCALES.map((locale) => (
                  <Radio key={locale} value={locale} className="prefs-choice" id={`prefs-language-${locale}`}>
                    <span className="radio-dot" aria-hidden="true" />
                    <span lang={locale}>{OWN_NAMES[locale]}</span>
                  </Radio>
                ))}
                <Text slot="description" className="prefs-hint">
                  {t('prefs.language.hint')}
                </Text>
              </RadioGroup>
            </section>

            <section className="prefs-section" aria-labelledby={`${titleId}-appearance`}>
              <h3 id={`${titleId}-appearance`} className="prefs-heading">
                {t('prefs.appearance.heading')}
              </h3>
              <RadioGroup className="prefs-choices" value={preferences?.theme ?? 'system'} onChange={(theme) => void save({ theme: theme as ThemeChoice })} id="prefs-theme">
                <Label className="field-label">{t('prefs.theme.label')}</Label>
                {(['system', 'light', 'dark'] as const).map((theme) => (
                  <Radio key={theme} value={theme} className="prefs-choice" id={`prefs-theme-${theme}`}>
                    <span className="radio-dot" aria-hidden="true" />
                    {t(`prefs.theme.${theme}`)}
                  </Radio>
                ))}
              </RadioGroup>
              <RadioGroup className="prefs-choices" value={preferences?.motion ?? 'system'} onChange={(motion) => void save({ motion: motion as MotionChoice })} id="prefs-motion">
                <Label className="field-label">{t('prefs.motion.label')}</Label>
                <Radio value="system" className="prefs-choice" id="prefs-motion-system">
                  <span className="radio-dot" aria-hidden="true" />
                  {t('prefs.motion.system', { state: t(systemReducesMotion() ? 'prefs.motion.state.reduced' : 'prefs.motion.state.full') })}
                </Radio>
                <Radio value="reduce" className="prefs-choice" id="prefs-motion-reduce">
                  <span className="radio-dot" aria-hidden="true" />
                  {t('prefs.motion.reduce')}
                </Radio>
                <Text slot="description" className="prefs-hint">
                  {t('prefs.motion.hint')}
                </Text>
              </RadioGroup>
            </section>

            <section className="prefs-section" aria-labelledby={`${titleId}-sidebar`}>
              <h3 id={`${titleId}-sidebar`} className="prefs-heading">
                {t('prefs.sidebar.heading')}
              </h3>
              <Checkbox
                className="prefs-choice"
                id="prefs-sidebar-empty"
                isSelected={preferences?.showEmptySidebarSections ?? false}
                onChange={(showEmptySidebarSections) => void save({ showEmptySidebarSections })}
              >
                <span className="check-box" aria-hidden="true" />
                <span>{t('prefs.sidebar.showEmpty')}</span>
              </Checkbox>
              <p className="prefs-hint">{t('prefs.sidebar.showEmptyHint')}</p>
              <Checkbox
                className="prefs-choice"
                id="prefs-sidebar-counts"
                isSelected={preferences?.showSidebarCounts ?? false}
                onChange={(showSidebarCounts) => void save({ showSidebarCounts })}
              >
                <span className="check-box" aria-hidden="true" />
                <span>{t('prefs.sidebar.showCounts')}</span>
              </Checkbox>
              <p className="prefs-hint">{t('prefs.sidebar.showCountsHint')}</p>
              <div className="prefs-range-block">
                <label className="field-label" htmlFor="prefs-cover-width">
                  {t('prefs.covers.label')}
                </label>
                <div className="prefs-range-row">
                  <input
                    id="prefs-cover-width"
                    className="prefs-range"
                    type="range"
                    min={COVER_MIN_WIDTH}
                    max={COVER_MAX_WIDTH}
                    step={4}
                    value={clampCoverWidth(preferences?.coverWidth ?? COVER_DEFAULT_WIDTH)}
                    onChange={(event) => void save({ coverWidth: clampCoverWidth(Number(event.currentTarget.value)) })}
                  />
                  <span className="prefs-range-value">
                    {t('prefs.covers.value', { width: clampCoverWidth(preferences?.coverWidth ?? COVER_DEFAULT_WIDTH) })}
                  </span>
                  <Button
                    className="button small"
                    isDisabled={clampCoverWidth(preferences?.coverWidth ?? COVER_DEFAULT_WIDTH) === COVER_DEFAULT_WIDTH}
                    onPress={() => void save({ coverWidth: COVER_DEFAULT_WIDTH })}
                  >
                    {t('prefs.covers.reset')}
                  </Button>
                </div>
                <p className="prefs-hint">{t('prefs.covers.hint')}</p>
              </div>
            </section>

            <section className="prefs-section" aria-labelledby={`${titleId}-annotations`}>
              <h3 id={`${titleId}-annotations`} className="prefs-heading">
                {t('prefs.annotations.heading')}
              </h3>
              <form
                className="prefs-author"
                onSubmit={(event) => {
                  event.preventDefault();
                  void save({ annotationAuthor: author }, author.trim() === '' ? t('prefs.author.cleared') : t('prefs.author.saved')).then(() => {
                    if (focusIsLost()) {
                      authorInput.current?.focus();
                    }
                  });
                }}
              >
                <TextField className="field grow" value={author} onChange={setAuthor} maxLength={128} id="prefs-author">
                  <Label className="field-label">{t('prefs.author.label')}</Label>
                  <Input ref={authorInput} className="text-input" placeholder={view?.systemAuthor ?? ''} />
                  <Text slot="description" className="prefs-hint">
                    {view?.systemAuthor !== null && view?.systemAuthor !== undefined ? t('prefs.author.hint', { user: view.systemAuthor }) : t('prefs.author.hint.nouser')}
                  </Text>
                </TextField>
                <Button type="submit" className="button" id="prefs-author-save" isDisabled={author.trim() === (preferences?.annotationAuthor ?? '')}>
                  {t('prefs.author.save')}
                </Button>
              </form>
            </section>

            <section className="prefs-section" aria-labelledby={`${titleId}-ocr`}>
              <h3 id={`${titleId}-ocr`} className="prefs-heading">
                {t('prefs.ocr.heading')}
              </h3>
              {ocr !== null && verified.length === 0 ? (
                <p className="prefs-hint">{t('prefs.ocr.none')}</p>
              ) : (
                <CheckboxGroup
                  className="prefs-choices"
                  value={preferences?.ocrLanguages ?? []}
                  onChange={(languages) => {
                    // In the order of the models, which is the order they are tried in; never none.
                    const ordered = verified.map((model) => model.code).filter((code) => languages.includes(code));
                    if (ordered.length > 0) {
                      void save({ ocrLanguages: ordered });
                    }
                  }}
                  id="prefs-ocr"
                >
                  <Label className="field-label">{t('prefs.ocr.label')}</Label>
                  {verified.map((model) => (
                    <Checkbox key={model.code} value={model.code} className="prefs-choice" id={`prefs-ocr-${model.code}`}>
                      <span className="check-box" aria-hidden="true" />
                      <span>{name(model.bcp47, model.code)}</span>
                      <span className="muted">
                        {t('prefs.ocr.verified')} · {formatBytes(model.bytes)}
                      </span>
                    </Checkbox>
                  ))}
                  <Text slot="description" className="prefs-hint">
                    {t('prefs.ocr.hint')}
                    {ocr?.reading !== null && ocr?.reading !== undefined ? ` ${t('prefs.ocr.reading', { languages: ocr.reading })}` : ''}
                  </Text>
                </CheckboxGroup>
              )}
              {unusable.length > 0 ? (
                <ul className="prefs-list">
                  {unusable.map((model) => (
                    <li key={model.code}>
                      {name(model.bcp47, model.code)}: <span className="muted">{model.bcp47 === null ? t('prefs.ocr.unknown') : t('prefs.ocr.unverified')}</span>
                    </li>
                  ))}
                </ul>
              ) : null}
              {ocr?.folder !== null && ocr?.folder !== undefined ? <p className="prefs-hint">{t('prefs.ocr.folder', { folder: ocr.folder })}</p> : null}
              {fetchable.length > 0 ? (
                <p className="prefs-hint">
                  {t('prefs.ocr.add', {
                    codes: fetchable.map((model) => name(model.bcp47, model.code)).join(', '),
                    command: `tools\\fetch-ocr.ps1 -Languages ${fetchable[0].code}`,
                  })}
                </p>
              ) : null}
            </section>

            <section className="prefs-section" aria-labelledby={`${titleId}-cache`} aria-busy={cache === undefined || clearing}>
              <h3 id={`${titleId}-cache`} className="prefs-heading">
                {t('prefs.cache.heading')}
              </h3>
              <p className="prefs-hint">{t('prefs.cache.intro')}</p>
              {cache === undefined ? (
                <p className="prefs-hint">{t('prefs.cache.loading')}</p>
              ) : cache === null ? (
                <p className="prefs-hint">{t('prefs.cache.noLibrary')}</p>
              ) : (
                <>
                  <table className="prefs-table" id="prefs-cache">
                    <thead>
                      <tr>
                        <th scope="col">{t('prefs.cache.part')}</th>
                        <th scope="col" className="number">
                          {t('prefs.cache.size')}
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      <tr data-part="lines">
                        <th scope="row">
                          {t('prefs.cache.lines')} <span className="muted">· {tn('prefs.cache.pages', cache.linePages)}</span>
                        </th>
                        <td className="number">{formatBytes(cache.lineBytes)}</td>
                      </tr>
                      <tr data-part="ocr">
                        <th scope="row">
                          {t('prefs.cache.ocr')} <span className="muted">· {tn('prefs.cache.ocrLines', cache.ocrLines)}</span>
                        </th>
                        <td className="number">{formatBytes(cache.ocrBytes)}</td>
                      </tr>
                      <tr data-part="fulltext">
                        <th scope="row">{t('prefs.cache.fulltext')}</th>
                        <td className="number">{formatBytes(cache.fulltextBytes)}</td>
                      </tr>
                      <tr data-part="flow">
                        <th scope="row">{t('prefs.cache.flow')}</th>
                        <td className="number">{formatBytes(cache.flowBytes)}</td>
                      </tr>
                      <tr data-part="thumbnails">
                        <th scope="row">{t('prefs.cache.thumbnails')}</th>
                        <td className="number">{formatBytes(cache.thumbnailBytes)}</td>
                      </tr>
                      <tr data-part="database">
                        <th scope="row">{t('prefs.cache.database')}</th>
                        <td className="number">{formatBytes(cache.databaseBytes)}</td>
                      </tr>
                    </tbody>
                    <tfoot>
                      <tr data-part="total">
                        <th scope="row">{t('prefs.cache.total', { folder: cache.folder })}</th>
                        <td className="number">{formatBytes(cache.totalBytes)}</td>
                      </tr>
                    </tfoot>
                  </table>
                  {confirming ? (
                    <div className="prefs-confirm" role="group" aria-labelledby={`${titleId}-confirm`}>
                      <p id={`${titleId}-confirm`}>
                        {t('prefs.cache.confirm', { pages: tn('prefs.cache.pages', cache.linePages), lines: tn('prefs.cache.ocrLines', cache.ocrLines) })}
                      </p>
                      <div className="modal-actions">
                        <Button className="button" onPress={() => setConfirming(false)} id="prefs-cache-cancel" autoFocus>
                          {t('prefs.cache.confirm.no')}
                        </Button>
                        <Button className="button danger" onPress={() => void clearCache()} isDisabled={clearing} id="prefs-cache-confirm">
                          {t('prefs.cache.confirm.yes')}
                        </Button>
                      </div>
                    </div>
                  ) : (
                    <Button className="button" onPress={() => setConfirming(true)} isDisabled={clearing} id="prefs-cache-clear">
                      {t('prefs.cache.clear')}
                    </Button>
                  )}
                </>
              )}
            </section>
            </div>
          </ScrollFrame>

          <footer className="prefs-footer">
            <p className={`prefs-status${status?.failed === true ? ' failed' : ''}`} role="status" aria-live="polite" id="prefs-status">
              {status?.text ?? (view?.path !== null && view?.path !== undefined ? t('prefs.kept', { path: view.path }) : '')}
            </p>
          </footer>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}
