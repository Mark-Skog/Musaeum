/**
 * The English catalog: each area of `pt-BR/` in `en/`, typed by its Portuguese twin, so that a key
 * missing here, or one that is only here, does not compile.
 */

import { common } from './en/common';
import { library } from './en/library';
import { preferences } from './en/preferences';
import { batch } from './en/batch';
import { details } from './en/details';
import { dialogs } from './en/dialogs';
import { sidebar } from './en/sidebar';
import { taxonomy } from './en/taxonomy';
import { exchange } from './en/exchange';
import { annot } from './en/annot';
import { reader } from './en/reader';
import type { Messages } from './pt-BR';

export const EN_AREAS = { common, batch, details, dialogs, sidebar, taxonomy, exchange, annot, reader, library, preferences };

export const en: Messages = { ...common, ...batch, ...details, ...dialogs, ...sidebar, ...taxonomy, ...exchange, ...annot, ...reader, ...library, ...preferences };
