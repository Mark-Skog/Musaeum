// Decides whether an SPDX license expression can be met using only the licenses on a list.
//
// `A OR B` is a choice, so one allowed side is enough. `A AND B` applies both, so both have to be
// allowed. `A WITH exception` is its own license, and has to be on the list as a whole.

function tokenize(expression) {
  return expression.match(/\(|\)|[^\s()]+/g) ?? [];
}

/** Whether `expression` can be satisfied with `allowed`. A malformed expression cannot. */
export function isSatisfiable(expression, allowed) {
  const tokens = tokenize(expression);
  let at = 0;

  function license() {
    const token = tokens[at++];
    if (token === undefined || token === ')' || token === 'AND' || token === 'OR') {
      throw new Error(`expected a license in "${expression}"`);
    }
    if (token === '(') {
      const value = or();
      if (tokens[at++] !== ')') {
        throw new Error(`unbalanced parentheses in "${expression}"`);
      }
      return value;
    }
    if (tokens[at] === 'WITH') {
      const exception = tokens[at + 1];
      at += 2;
      return allowed.has(`${token} WITH ${exception}`);
    }
    return allowed.has(token);
  }

  function and() {
    let value = license();
    while (tokens[at] === 'AND') {
      at++;
      const right = license();
      value = value && right;
    }
    return value;
  }

  function or() {
    let value = and();
    while (tokens[at] === 'OR') {
      at++;
      const right = and();
      value = value || right;
    }
    return value;
  }

  try {
    const value = or();
    return at === tokens.length && value;
  } catch {
    return false;
  }
}
