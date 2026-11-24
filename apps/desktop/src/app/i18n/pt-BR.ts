/**
 * O catálogo em português do Brasil, a primeira língua do Musaeum: cada área num arquivo de
 * `pt-BR/`, e as chaves de uma área começam pelo nome dela, para não se cruzarem.
 */

import { common } from './pt-BR/common';
import { library } from './pt-BR/library';
import { preferences } from './pt-BR/preferences';
import { batch } from './pt-BR/batch';
import { details } from './pt-BR/details';
import { dialogs } from './pt-BR/dialogs';
import { sidebar } from './pt-BR/sidebar';
import { taxonomy } from './pt-BR/taxonomy';
import { exchange } from './pt-BR/exchange';
import { annot } from './pt-BR/annot';
import { reader } from './pt-BR/reader';

/** Each area, for the test that holds that no key is in two of them. */
export const PT_AREAS = { common, batch, details, dialogs, sidebar, taxonomy, exchange, annot, reader, library, preferences };

export const ptBR = { ...common, ...batch, ...details, ...dialogs, ...sidebar, ...taxonomy, ...exchange, ...annot, ...reader, ...library, ...preferences };

export type MessageKey = keyof typeof ptBR;
export type Messages = Record<MessageKey, string>;
