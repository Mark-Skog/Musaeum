import { describe, expect, it } from 'vitest';

import { Samples, SegmentRecorder, summarize } from './metrics';
import { buildScenario, ScenarioPlayer, type SegmentContext } from './scenarios';

describe('summarize', () => {
  it('uses nearest-rank percentiles, like the Rust side', () => {
    const values = Float32Array.from({ length: 100 }, (_, index) => 100 - index);
    expect(summarize(values)).toEqual({
      count: 100,
      mean: 50.5,
      p50: 50,
      p95: 95,
      p99: 99,
      max: 100,
    });
  });

  it('handles empty and single-value input', () => {
    expect(summarize(new Float32Array(0)).count).toBe(0);
    expect(summarize(new Float32Array([7])).p99).toBe(7);
  });
});

describe('Samples', () => {
  it('drops samples past its capacity instead of growing', () => {
    const samples = new Samples(2);
    samples.add(1);
    samples.add(2);
    samples.add(3);
    expect(samples.count).toBe(2);
    expect(samples.summary().max).toBe(2);
  });

  it('counts values above a limit', () => {
    const samples = new Samples(4);
    [1, 5, 9].forEach((value) => samples.add(value));
    expect(samples.countAbove(4)).toBe(2);
  });
});

describe('SegmentRecorder', () => {
  it('reports frames over the 120 Hz budget and frames that missed the display', () => {
    const recorder = new SegmentRecorder('test');
    [5, 8, 9, 20].forEach((value) => recorder.frameWork.add(value));
    [13, 13, 40].forEach((value) => recorder.frameInterval.add(value));
    const report = recorder.report(13.3);
    expect(report.workOver120HzBudget).toBe(2);
    expect(report.missedFrames).toBe(1);
    expect(report.exactCoverage).toBe(0);
  });
});

const CONTEXT: SegmentContext = { minScrollPt: 0, maxScrollPt: 100_000, viewHeightPt: 500 };

function runSegment(name: string): { name: string; positions: number[]; resting: boolean[] } {
  const scenario = buildScenario('full');
  const segment = scenario.find((candidate) => candidate.name === name)!;
  const positions: number[] = [];
  const resting: boolean[] = [];
  for (let at = 0; at < segment.durationMs; at += 8) {
    positions.push(segment.position(at, 1000, CONTEXT));
    resting.push(segment.isResting(at));
  }
  return { name, positions, resting };
}

describe('scenarios', () => {
  it('are deterministic, so two runs ask for exactly the same thing', () => {
    expect(runSegment('teleports').positions).toEqual(runSegment('teleports').positions);
    expect(runSegment('natural-flicks').positions).toEqual(runSegment('natural-flicks').positions);
  });

  it('stay inside the document', () => {
    for (const segment of buildScenario('full')) {
      for (let at = 0; at < segment.durationMs; at += 16) {
        const position = segment.position(at, 99_900, CONTEXT);
        expect(position).toBeGreaterThanOrEqual(0);
        expect(position).toBeLessThanOrEqual(100_000);
      }
    }
  });

  it('move at the requested speed', () => {
    const { positions } = runSegment('extreme-30vh');
    // 30 viewports per second of 500 pt each, over 800 ms.
    expect(positions[100] - positions[0]).toBeCloseTo(30 * 500 * 0.8, 0);
  });

  it('flick smoothly from a start, without jumps, then rest', () => {
    const { positions, resting } = runSegment('natural-flicks');
    for (let at = 1; at < positions.length; at++) {
      expect(positions[at]).toBeGreaterThanOrEqual(positions[at - 1]);
      // Never faster than the peak of 6 viewports per second over 8 ms.
      expect(positions[at] - positions[at - 1]).toBeLessThan(6 * 500 * 0.008 + 1e-6);
    }
    expect(resting.some(Boolean)).toBe(true);
    expect(resting.some((value) => !value)).toBe(true);
    // During a rest the position holds still.
    const firstRest = resting.indexOf(true);
    expect(positions[firstRest + 5]).toBe(positions[firstRest]);
  });

  it('jump around widely when teleporting', () => {
    const unique = new Set(runSegment('teleports').positions);
    expect(unique.size).toBe(20);
    const spread = Math.max(...unique) - Math.min(...unique);
    expect(spread).toBeGreaterThan(50_000);
  });

  it('have a smoke variant short enough to run in a test suite', () => {
    const scenario = buildScenario('smoke');
    const total = scenario.reduce((sum, segment) => sum + segment.durationMs, 0);
    expect(total).toBeLessThan(3000);
    expect(scenario[0].name).toBe('open');
  });

  it('are shorter in the quick variant', () => {
    const total = (name: 'full' | 'quick'): number =>
      buildScenario(name).reduce((sum, segment) => sum + segment.durationMs, 0);
    expect(total('quick')).toBeLessThan(total('full') * 0.65);
  });
});

describe('ScenarioPlayer', () => {
  it('moves through segments as time passes, then finishes', () => {
    const scenario = buildScenario('quick');
    const player = new ScenarioPlayer(scenario);
    const total = scenario.reduce((sum, segment) => sum + segment.durationMs, 0);

    let position = 0;
    let visited = new Set<number>();
    let now = 1000;
    while (!player.finished) {
      visited.add(player.currentIndex);
      const sampled = player.sample(now, position, CONTEXT);
      if (sampled === null) {
        break;
      }
      position = sampled;
      now += 8;
      expect(now).toBeLessThan(1000 + total + 1000);
    }
    expect(visited.size).toBe(scenario.length);
    expect(player.sample(now, position, CONTEXT)).toBeNull();
  });

  it('starts each segment from where the previous one stopped', () => {
    const player = new ScenarioPlayer(buildScenario('quick'));
    player.sample(0, 1234, CONTEXT);
    expect(player.sample(100, 1234, CONTEXT)).toBe(1234);
  });
});
