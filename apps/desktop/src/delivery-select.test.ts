import { describe, expect, it } from 'vitest';

import { type DeliveryEvidence, planDelivery } from './delivery-select';
import type { ProbeOutcome } from './probe';

function outcome(overrides: Partial<ProbeOutcome> = {}): ProbeOutcome {
  return {
    label: 'probe',
    transfer: 'accepted',
    detachedAfterTransfer: true,
    received: true,
    workerSeesHostWrites: true,
    hostSeesWorkerWrites: null,
    workerHasWebviewApi: false,
    ...overrides,
  };
}

/** What WebView2 156 was measured to do: everything works. */
function working(overrides: Partial<DeliveryEvidence> = {}): DeliveryEvidence {
  return {
    hasWebviewApi: true,
    hostBufferError: null,
    mainThreadSeesHostWrites: true,
    crossOriginIsolated: true,
    hasAtomicsWaitAsync: true,
    workerProbes: [outcome(), outcome({ hostSeesWorkerWrites: true })],
    ...overrides,
  };
}

describe('automatic delivery selection', () => {
  it('prefers Path C when transferred buffers stay live in both directions', () => {
    expect(planDelivery('auto', working())).toEqual({ candidates: ['c', 'b', 'a'], rejected: [] });
  });

  it('falls back to Path B when the worker gets a copy instead of live memory', () => {
    const plan = planDelivery(
      'auto',
      working({
        workerProbes: [
          outcome({ workerSeesHostWrites: false }),
          outcome({ workerSeesHostWrites: false, hostSeesWorkerWrites: false }),
        ],
      }),
    );
    expect(plan.candidates).toEqual(['b', 'a']);
    expect(plan.rejected.map((rejected) => rejected.path)).toEqual(['c']);
  });

  it('falls back to Path B when the runtime refuses the transfer', () => {
    const plan = planDelivery(
      'auto',
      working({
        workerProbes: [
          outcome({ transfer: 'DataCloneError: no', received: false, workerSeesHostWrites: false }),
          outcome({ transfer: 'DataCloneError: no', received: false, workerSeesHostWrites: false }),
        ],
      }),
    );
    expect(plan.candidates).toEqual(['b', 'a']);
  });

  it('falls back to Path B when only the host cannot see what the worker writes', () => {
    const plan = planDelivery(
      'auto',
      working({ workerProbes: [outcome(), outcome({ hostSeesWorkerWrites: false })] }),
    );
    expect(plan.candidates).toEqual(['b', 'a']);
  });

  it('uses Path A without WebView2 at all', () => {
    const plan = planDelivery(
      'auto',
      working({
        hasWebviewApi: false,
        mainThreadSeesHostWrites: false,
        workerProbes: [],
      }),
    );
    expect(plan.candidates).toEqual(['a']);
    expect(plan.rejected.map((rejected) => rejected.path)).toEqual(['c', 'b']);
  });

  it('uses Path A when the host cannot create shared buffers', () => {
    const plan = planDelivery(
      'auto',
      working({
        hostBufferError: 'this WebView2 runtime lacks shared buffers',
        mainThreadSeesHostWrites: false,
        workerProbes: [],
      }),
    );
    expect(plan.candidates).toEqual(['a']);
    expect(plan.rejected[0].reason).toContain('lacks shared buffers');
  });

  it('keeps Path C when only the requirements of Path B are missing', () => {
    const plan = planDelivery('auto', working({ crossOriginIsolated: false }));
    expect(plan.candidates).toEqual(['c', 'a']);
    expect(plan.rejected.map((rejected) => rejected.path)).toEqual(['b']);
  });

  it('skips Path B without cross-origin isolation once Path C is out too', () => {
    const plan = planDelivery(
      'auto',
      working({ crossOriginIsolated: false, workerProbes: [outcome(), outcome()] }),
    );
    expect(plan.candidates).toEqual(['a']);
  });

  it('skips Path B when the main thread does not see the host', () => {
    const plan = planDelivery(
      'auto',
      working({ mainThreadSeesHostWrites: false, workerProbes: [] }),
    );
    expect(plan.candidates).toEqual(['a']);
  });

  it('skips Path B when an idle worker could not be woken', () => {
    const plan = planDelivery('auto', working({ hasAtomicsWaitAsync: false, workerProbes: [] }));
    expect(plan.candidates).toEqual(['a']);
  });

  it('always leaves Path A as the last resort', () => {
    const nothing: DeliveryEvidence = {
      hasWebviewApi: false,
      hostBufferError: 'no',
      mainThreadSeesHostWrites: false,
      crossOriginIsolated: false,
      hasAtomicsWaitAsync: false,
      workerProbes: [],
    };
    expect(planDelivery('auto', nothing).candidates).toEqual(['a']);
  });
});

describe('an explicitly requested path', () => {
  it('is the only candidate when it works', () => {
    expect(planDelivery('b', working()).candidates).toEqual(['b']);
    expect(planDelivery('c', working()).candidates).toEqual(['c']);
    expect(planDelivery('a', working()).candidates).toEqual(['a']);
  });

  it('fails instead of quietly measuring a different path', () => {
    const plan = planDelivery('c', working({ workerProbes: [] }));
    expect(plan.candidates).toEqual([]);
    expect(plan.rejected.map((rejected) => rejected.path)).toEqual(['c']);
  });
});
