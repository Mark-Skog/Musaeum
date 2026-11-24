/**
 * The annotations' way in and out, wherever it is offered: JSON-LD both ways, and the study reports.
 * One action runs at a time; what it did, or why it could not, stays under the buttons.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Button, Heading } from 'react-aria-components';

import {
  EXCHANGE_ACTIONS,
  type ExchangeAction,
  type ExchangeDone,
  type ExchangeOutcome,
  type ExchangeWhere,
  reasonText,
  runExchange,
} from './exchange';
import { t as translate, useT } from './i18n';

export interface ExchangeControls {
  busy: ExchangeAction | null;
  report: { text: string; failed: boolean } | null;
  /** Runs an action; under a test, with the path the dialog would have given and a fixed time. */
  run(action: ExchangeAction, path?: string, generatedAt?: number): Promise<ExchangeOutcome>;
}

/**
 * Where each surface's controls are, for the tests that drive the window: the reader's panel, the
 * details of one work, the details of several, the library's menu.
 */
export const exchangeHooks = new Map<string, ExchangeControls['run']>();

export function useExchange(where: ExchangeWhere | null, onDone?: (action: ExchangeAction, done: ExchangeDone) => void, hook?: string): ExchangeControls {
  const [busy, setBusy] = useState<ExchangeAction | null>(null);
  const [report, setReport] = useState<{ text: string; failed: boolean } | null>(null);
  const key = JSON.stringify(where);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const place = useMemo(() => where, [key]);
  const done = useRef(onDone);
  done.current = onDone;
  useEffect(() => setReport(null), [key]);
  const run = useCallback(
    async (action: ExchangeAction, path?: string, generatedAt?: number): Promise<ExchangeOutcome> => {
      if (place === null) {
        return { ok: false, error: 'nada escolhido' };
      }
      setBusy(action);
      setReport(null);
      try {
        const result = await runExchange(action, place, path, generatedAt);
        if (result !== null) {
          setReport({ text: result.text, failed: false });
          done.current?.(action, result);
        }
        return { ok: true, done: result };
      } catch (reason: unknown) {
        const message = reasonText(reason);
        setReport({ text: translate('common.couldNot', { reason: message }), failed: true });
        return { ok: false, error: message };
      } finally {
        setBusy(null);
      }
    },
    [place],
  );
  useEffect(() => {
    if (hook === undefined) {
      return;
    }
    exchangeHooks.set(hook, run);
    return () => {
      if (exchangeHooks.get(hook) === run) {
        exchangeHooks.delete(hook);
      }
    };
  }, [hook, run]);
  return { busy, report, run };
}

export interface ExchangeSectionProps {
  heading: string;
  controls: ExchangeControls;
  /** The actions offered, in this order. */
  actions?: ExchangeAction[];
  /** What the status line says while nothing has run. */
  idle: string;
  /** Prefix of the buttons' ids, for the tests. */
  id: string;
  className?: string;
}

export function ExchangeSection({ heading, controls, actions, idle, id, className }: ExchangeSectionProps): React.JSX.Element {
  const t = useT();
  const shown = EXCHANGE_ACTIONS.filter((action) => actions === undefined || actions.includes(action.id));
  return (
    <section className={`pdf-exchange exchange-section${className !== undefined ? ` ${className}` : ''}`} aria-label={heading}>
      <Heading level={3} className="pdf-exchange-heading">
        {heading}
      </Heading>
      <div className="pdf-exchange-actions">
        {shown.map((action) => (
          <Button key={action.id} id={`${id}-${action.id}`} className="button small" isDisabled={controls.busy !== null} onPress={() => void controls.run(action.id)}>
            {t(action.label)}
          </Button>
        ))}
      </div>
      <p className={`exchange-status${controls.report?.failed === true ? ' failed' : ''}`} role="status" aria-live="polite" aria-busy={controls.busy !== null} data-busy={controls.busy ?? ''} id={`${id}-status`}>
        {controls.busy !== null ? t(EXCHANGE_ACTIONS.find((action) => action.id === controls.busy)?.busy ?? 'common.loading') : (controls.report?.text ?? idle)}
      </p>
    </section>
  );
}
