import assert from 'node:assert/strict';
import { test } from 'node:test';

import { isSatisfiable } from './spdx.mjs';

const allowed = new Set(['MIT', 'Apache-2.0', 'Apache-2.0 WITH LLVM-exception', 'BSD-3-Clause']);

test('a single license has to be on the list', () => {
  assert.equal(isSatisfiable('MIT', allowed), true);
  assert.equal(isSatisfiable('AGPL-3.0-only', allowed), false);
});

test('a choice needs one allowed side', () => {
  assert.equal(isSatisfiable('MIT OR AGPL-3.0-only', allowed), true);
  assert.equal(isSatisfiable('AGPL-3.0-only OR MIT', allowed), true);
  assert.equal(isSatisfiable('AGPL-3.0-only OR SSPL-1.0', allowed), false);
});

test('a conjunction needs both sides', () => {
  assert.equal(isSatisfiable('MIT AND BSD-3-Clause', allowed), true);
  assert.equal(isSatisfiable('MIT AND AGPL-3.0-only', allowed), false);
});

test('AND binds tighter than OR', () => {
  assert.equal(isSatisfiable('AGPL-3.0-only AND MIT OR Apache-2.0', allowed), true);
  assert.equal(isSatisfiable('MIT OR Apache-2.0 AND AGPL-3.0-only', allowed), true);
  assert.equal(isSatisfiable('(MIT OR Apache-2.0) AND AGPL-3.0-only', allowed), false);
});

test('an exception is part of the license it modifies', () => {
  assert.equal(isSatisfiable('Apache-2.0 WITH LLVM-exception', allowed), true);
  assert.equal(isSatisfiable('MIT WITH LLVM-exception', allowed), false);
  assert.equal(isSatisfiable('Apache-2.0 WITH LLVM-exception OR GPL-2.0-only', allowed), true);
});

test('anything malformed or missing is refused', () => {
  for (const expression of ['', 'OR', 'MIT OR', '(MIT', 'MIT)', 'MIT MIT', 'SEE LICENSE IN x.txt']) {
    assert.equal(isSatisfiable(expression, allowed), false, expression);
  }
});
