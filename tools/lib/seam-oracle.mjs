// Where the text after a cut belongs, by the browser's own layout of the uncut document.
//
// The normalizer cuts a long document into parts and repeats, in each part, the containers the cut
// went through. The reader draws each part in a frame of its own, so how the two meet is the
// reader's work, and the test cannot take it from the reader: this joins the two parts' documents
// back into one, lays it out in a hidden frame of the same width, and measures where the first box
// of the second part falls, against where the reader draws it.

/**
 * Runs in the reader's page (pass it as `(${seamOracle})(cut)`), with both parts loaded in
 * continuous reading. `cut` is the section that continues a document.
 */
export async function seamOracle(cut) {
  const XHTML = 'http://www.w3.org/1999/xhtml';
  const hooks = window.musaeumReaderE2E;
  const plan = hooks.plan(cut);
  const above = hooks.plan(cut - 1);
  if (plan === null || above === null || plan.part === 0 || plan.document !== above.document) {
    return { cut, measured: false, why: 'not a cut' };
  }
  const frameOf = (index) => document.querySelector(`.continuous-section[data-section="${index}"] iframe`);
  const frameA = frameOf(cut - 1);
  const frameB = frameOf(cut);
  const docA = frameA?.contentDocument ?? null;
  const docB = frameB?.contentDocument ?? null;
  if (docA?.body == null || docB?.body == null) {
    return { cut, measured: false, why: 'a part is not loaded' };
  }
  // The containers the normalizer repeated: all the steps of the start but the last.
  const depth = plan.start.length - 1;
  let containerB = docB.body;
  for (let level = 0; level < depth && containerB !== null; level++) {
    containerB = containerB.firstElementChild;
  }
  if (containerB === null) {
    return { cut, measured: false, why: 'the second part has no container' };
  }
  // The first box of the second part, measured in both layouts.
  let first = null;
  const walker = docB.createTreeWalker(containerB, NodeFilter.SHOW_ELEMENT);
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    const style = docB.defaultView.getComputedStyle(node);
    if (node.getBoundingClientRect().height > 0 && style.position !== 'absolute' && style.position !== 'fixed' && style.float === 'none') {
      first = node;
      break;
    }
  }
  if (first === null) {
    return { cut, measured: false, why: 'the second part has no box' };
  }

  const serializer = new XMLSerializer();
  const joined = new DOMParser().parseFromString(serializer.serializeToString(docA), 'application/xhtml+xml');
  if (joined.getElementsByTagName('parsererror').length > 0 || joined.body === null) {
    return { cut, measured: false, why: 'the first part does not parse again' };
  }
  // What the reader added at the end of the first part is not in the book.
  for (const probe of [...joined.getElementsByTagNameNS(XHTML, 'musaeum-seam')]) {
    probe.remove();
  }
  let containerJ = joined.body;
  for (let level = 0; level < depth && containerJ !== null; level++) {
    containerJ = containerJ.lastElementChild;
  }
  if (containerJ === null) {
    return { cut, measured: false, why: 'the first part has no container' };
  }
  first.setAttribute('data-musaeum-oracle', '');
  try {
    for (const node of [...containerB.childNodes]) {
      const copy = joined.importNode(node, true);
      // Nor are the edges the reader may have taken off the second part's own elements.
      if (copy.nodeType === Node.ELEMENT_NODE && copy.style !== undefined) {
        for (const property of ['margin-block-start', 'padding-block-start', 'border-block-start-width']) {
          if (copy.style.getPropertyPriority(property) === 'important') {
            copy.style.removeProperty(property);
          }
        }
      }
      containerJ.append(copy);
    }
  } finally {
    first.removeAttribute('data-musaeum-oracle');
  }

  const url = URL.createObjectURL(new Blob([serializer.serializeToString(joined)], { type: 'application/xhtml+xml' }));
  const box = frameA.getBoundingClientRect();
  const frame = document.createElement('iframe');
  frame.setAttribute('sandbox', frameA.getAttribute('sandbox') ?? 'allow-same-origin');
  frame.setAttribute('scrolling', 'no');
  frame.style.cssText = `position: fixed; top: 0; left: ${-box.width - 200}px; width: ${box.width}px; height: ${box.height}px; border: 0; visibility: hidden;`;
  try {
    await new Promise((resolve) => {
      frame.addEventListener('load', resolve, { once: true });
      frame.src = url;
      document.body.append(frame);
    });
    const doc = frame.contentDocument;
    await Promise.all([...doc.images].map((image) => (image.complete ? null : image.decode().catch(() => null))));
    await doc.fonts?.ready;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const mark = doc.querySelector('[data-musaeum-oracle]');
    if (mark === null) {
      return { cut, measured: false, why: 'the joined document lost the box' };
    }
    const expected = mark.getBoundingClientRect().top - doc.documentElement.getBoundingClientRect().top;
    const ours =
      frameB.getBoundingClientRect().top -
      frameA.getBoundingClientRect().top +
      (first.getBoundingClientRect().top - docB.documentElement.getBoundingClientRect().top);
    // What can make the two layouts differ other than the seam: images not drawn yet in the
    // reader, and columns, which balance the whole of their content.
    const pending = (target) => [...target.images].filter((image) => !image.complete || image.naturalHeight === 0).length;
    let columns = null;
    for (let node = first; node !== null && node.nodeType === Node.ELEMENT_NODE; node = node.parentElement) {
      const style = docB.defaultView.getComputedStyle(node);
      if (style.columnCount !== 'auto' || style.columnWidth !== 'auto') {
        columns = `${node.localName}: ${style.columnCount} × ${style.columnWidth}`;
        break;
      }
    }
    // And floats of the first part that hang past the end of its flow of blocks.
    const probe = docA.getElementsByTagNameNS(XHTML, 'musaeum-seam')[0] ?? null;
    const flowEnd = probe === null ? null : probe.getBoundingClientRect().top - docA.documentElement.getBoundingClientRect().top;
    let hanging = 0;
    let floatBottom = null;
    if (flowEnd !== null) {
      for (const element of docA.body.getElementsByTagName('*')) {
        if (docA.defaultView.getComputedStyle(element).float !== 'none') {
          const bottom = element.getBoundingClientRect().bottom - docA.documentElement.getBoundingClientRect().top;
          if (bottom > flowEnd + 0.5) {
            hanging++;
            floatBottom = Math.max(floatBottom ?? bottom, bottom);
          }
        }
      }
    }
    // When they differ: the first element of the first part drawn somewhere else in the two.
    let divergence = null;
    if (Math.abs(ours - expected) > 1) {
      const mine = docA.body.getElementsByTagName('*');
      const theirs = doc.body.getElementsByTagName('*');
      const count = Math.min(mine.length - (probe === null ? 0 : 1), theirs.length);
      const topOf = (element) => element.getBoundingClientRect().top - element.ownerDocument.documentElement.getBoundingClientRect().top;
      const differs = (index) => Math.abs(topOf(mine[index]) - topOf(theirs[index])) > 1;
      if (count > 0 && differs(count - 1)) {
        let low = 0;
        let high = count - 1;
        while (low < high) {
          const middle = (low + high) >> 1;
          if (differs(middle)) {
            high = middle;
          } else {
            low = middle + 1;
          }
        }
        const a = mine[low];
        const j = theirs[low];
        divergence = {
          index: low,
          of: count,
          element: `${a.localName}${a.getAttribute('class') ? `.${a.getAttribute('class')}` : ''}`,
          same: j.localName === a.localName,
          text: (a.textContent ?? '').trim().slice(0, 60),
          top: [topOf(a), topOf(j)],
          height: [a.getBoundingClientRect().height, j.getBoundingClientRect().height],
          previous: a.previousElementSibling === null ? null : `${a.previousElementSibling.localName}: ${a.previousElementSibling.getBoundingClientRect().height} / ${j.previousElementSibling?.getBoundingClientRect().height}`,
        };
      }
    }
    return {
      cut,
      measured: true,
      ours,
      expected,
      delta: ours - expected,
      box: `${first.localName}${first.getAttribute('class') ? `.${first.getAttribute('class')}` : ''}`,
      pendingImages: { reader: pending(docA), joined: pending(doc) },
      columns,
      firstPart: { height: docA.body.getBoundingClientRect().height, flowEnd, hanging, floatBottom },
      divergence,
    };
  } finally {
    frame.remove();
    URL.revokeObjectURL(url);
  }
}

/** How far the reader may put the text after a cut from where the uncut document has it, in px. */
export const SEAM_TOLERANCE_PX = 1;
