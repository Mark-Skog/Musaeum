/**
 * Decides which delivery path a session uses, from what the runtime was seen to do.
 *
 * Path C is the fastest and leans on behaviour Microsoft does not document, so it needs the
 * most proof. Path B only uses documented APIs, but needs cross-origin isolation. Path A
 * needs nothing beyond the custom protocol, which makes it the last resort everywhere and
 * the only path on systems without WebView2.
 */

import { directPathAvailable, type ProbeOutcome } from './probe';
import type { DeliveryPath, DeliveryRequest } from './protocol';

/** What was observed at startup. Everything here comes from trying it, not from assuming. */
export interface DeliveryEvidence {
  /** Whether the page has `chrome.webview`, meaning it runs on WebView2. */
  hasWebviewApi: boolean;
  /** Why the host could not create shared buffers, or null if it could. */
  hostBufferError: string | null;
  /** Whether the main thread sees what the host writes into a shared buffer. */
  mainThreadSeesHostWrites: boolean;
  crossOriginIsolated: boolean;
  hasAtomicsWaitAsync: boolean;
  /** One outcome per probe buffer. Empty when no worker was tested. */
  workerProbes: ProbeOutcome[];
}

export interface RejectedPath {
  path: DeliveryPath;
  reason: string;
}

export interface DeliveryPlan {
  /** Paths to try, best first. */
  candidates: DeliveryPath[];
  /** Paths ruled out, and why. */
  rejected: RejectedPath[];
}

/** Why Path C cannot be used, or null if it can. */
function directPathProblem(evidence: DeliveryEvidence): string | null {
  if (!evidence.hasWebviewApi) {
    return 'this is not WebView2: chrome.webview is missing';
  }
  if (evidence.hostBufferError !== null) {
    return evidence.hostBufferError;
  }
  if (!directPathAvailable(evidence.workerProbes)) {
    return `a transferred buffer did not stay live in the worker: ${JSON.stringify(evidence.workerProbes)}`;
  }
  return null;
}

/** Why Path B cannot be used, or null if it can. */
function ringPathProblem(evidence: DeliveryEvidence): string | null {
  if (!evidence.hasWebviewApi) {
    return 'this is not WebView2: chrome.webview is missing';
  }
  if (evidence.hostBufferError !== null) {
    return evidence.hostBufferError;
  }
  if (!evidence.crossOriginIsolated) {
    return 'the page is not cross-origin isolated, so it has no SharedArrayBuffer';
  }
  if (!evidence.hasAtomicsWaitAsync) {
    return 'Atomics.waitAsync is missing, so an idle worker could not be woken';
  }
  if (!evidence.mainThreadSeesHostWrites) {
    return 'the main thread does not see what the host writes into a shared buffer';
  }
  return null;
}

/**
 * Turns the evidence into a list of paths to try. An explicit request narrows the list to
 * that one path, which fails rather than falls back, because whoever asked for it is
 * measuring it.
 */
export function planDelivery(request: DeliveryRequest, evidence: DeliveryEvidence): DeliveryPlan {
  const problems: Record<DeliveryPath, string | null> = {
    c: directPathProblem(evidence),
    b: ringPathProblem(evidence),
    a: null,
  };
  const order: DeliveryPath[] = request === 'auto' ? ['c', 'b', 'a'] : [request];
  const plan: DeliveryPlan = { candidates: [], rejected: [] };
  for (const path of order) {
    const reason = problems[path];
    if (reason === null) {
      plan.candidates.push(path);
    } else {
      plan.rejected.push({ path, reason });
    }
  }
  return plan;
}
