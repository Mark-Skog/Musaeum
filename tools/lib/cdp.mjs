// Driving the real app window through the WebView2's own debugging protocol (CDP): starting the
// app with its debugging port open, talking to its pages, and giving them trusted input.
//
// Every click and key goes into a page through Chromium's input pipeline, as trusted events:
// nothing moves the mouse or sends keys to whatever window has the focus, and nothing captures the
// screen.

import { execFileSync, spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';

/** Seconds since anyone last touched the keyboard or the mouse. */
export function idleSeconds() {
  const script =
    "Add-Type -Name IdleCdp -Namespace Musaeum -MemberDefinition '" +
    '[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)] ' +
    'public struct LII { public uint cbSize; public uint dwTime; } ' +
    '[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LII p);' +
    "'; $l = New-Object Musaeum.IdleCdp+LII; $l.cbSize = 8; [void][Musaeum.IdleCdp]::GetLastInputInfo([ref]$l); " +
    '[int](([Environment]::TickCount - $l.dwTime) / 1000)';
  // Hidden: on a machine someone is using, no console may flash over what they are doing.
  const output = execFileSync('powershell', ['-NoProfile', '-NonInteractive', '-Command', script], {
    encoding: 'utf8',
    windowsHide: true,
  });
  return Number(output.trim());
}

/** One connection to a page, or a frame of one, through the DevTools protocol. */
export class Cdp {
  constructor(url) {
    this.url = url;
    this.next = 1;
    this.pending = new Map();
    this.listeners = new Set();
  }

  async connect() {
    this.socket = new WebSocket(this.url);
    await new Promise((resolveOpen, reject) => {
      this.socket.addEventListener('open', resolveOpen, { once: true });
      this.socket.addEventListener('error', () => reject(new Error('no CDP connection')), { once: true });
    });
    this.socket.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      const waiting = message.id === undefined ? undefined : this.pending.get(message.id);
      if (waiting === undefined) {
        for (const listener of this.listeners) {
          listener(message);
        }
        return;
      }
      this.pending.delete(message.id);
      if (message.error) {
        waiting.reject(new Error(`${message.error.message} (${message.error.code})`));
      } else {
        waiting.resolve(message.result);
      }
    });
  }

  /** Every call has a deadline: an answer that never comes fails the step instead of the whole run. */
  send(method, params = {}, timeoutMs = 30_000, sessionId = undefined) {
    const id = this.next++;
    this.socket.send(JSON.stringify({ id, method, params, ...(sessionId !== undefined ? { sessionId } : {}) }));
    return new Promise((resolveSend, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} got no answer in ${timeoutMs} ms`));
      }, timeoutMs);
      const settled = (finish) => (value) => {
        clearTimeout(timer);
        finish(value);
      };
      this.pending.set(id, { resolve: settled(resolveSend), reject: settled(reject) });
    });
  }

  async evaluate(expression, timeoutMs = 30_000, where = {}) {
    const result = await this.send(
      'Runtime.evaluate',
      {
        expression,
        awaitPromise: true,
        returnByValue: true,
        ...(where.contextId !== undefined ? { contextId: where.contextId } : {}),
      },
      timeoutMs,
      where.sessionId,
    );
    if (result.exceptionDetails) {
      throw new Error(`page threw: ${JSON.stringify(result.exceptionDetails).slice(0, 600)}`);
    }
    return result.result.value;
  }

  close() {
    this.socket?.close();
  }
}

const KEYS = {
  ArrowRight: { code: 'ArrowRight', keyCode: 39 },
  ArrowLeft: { code: 'ArrowLeft', keyCode: 37 },
  ArrowDown: { code: 'ArrowDown', keyCode: 40 },
  ArrowUp: { code: 'ArrowUp', keyCode: 38 },
  PageDown: { code: 'PageDown', keyCode: 34 },
  PageUp: { code: 'PageUp', keyCode: 33 },
  Escape: { code: 'Escape', keyCode: 27 },
  Enter: { code: 'Enter', keyCode: 13, text: '\r' },
  Home: { code: 'Home', keyCode: 36 },
  End: { code: 'End', keyCode: 35 },
  Tab: { code: 'Tab', keyCode: 9 },
  ',': { code: 'Comma', keyCode: 188, text: ',' },
};

/** A key pressed and released, as a person would. */
export async function press(cdp, key, modifiers = 0) {
  const named = KEYS[key];
  const letter = key.length === 1 ? key : null;
  const fields = named ?? {
    code: `Key${letter.toUpperCase()}`,
    keyCode: letter.toUpperCase().charCodeAt(0),
    text: letter,
  };
  const base = {
    key,
    code: fields.code,
    windowsVirtualKeyCode: fields.keyCode,
    nativeVirtualKeyCode: fields.keyCode,
    modifiers,
  };
  // With Ctrl, Alt or the Windows key held, a key is a command and types nothing: sending its text
  // would write the letter into whatever has the focus.
  const types = fields.text !== undefined && (modifiers & (1 | 2 | 4)) === 0;
  await cdp.send('Input.dispatchKeyEvent', {
    type: types ? 'keyDown' : 'rawKeyDown',
    ...base,
    ...(types ? { text: fields.text, unmodifiedText: fields.text } : {}),
  });
  await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
}

/**
 * A click in the middle of the first element `selector` finds, scrolled into view first, with the
 * keys `modifiers` holds down (CDP's bits: 1 Alt, 2 Ctrl, 4 Meta, 8 Shift), as Ctrl+click does.
 */
export async function click(cdp, selector, modifiers = 0) {
  const box = await cdp.evaluate(`(async () => {
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element) return null;
    const scrollers = [];
    for (let node = element.parentElement; node !== null; node = node.parentElement) {
      if (node.scrollHeight > node.clientHeight || node.scrollWidth > node.clientWidth) {
        scrollers.push([node, node.scrollTop, node.scrollLeft]);
      }
    }
    // As little as it takes: a click should not move what it is aimed at.
    element.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    // The scroll event comes with the next frame.
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    if (scrollers.some(([node, top, left]) => node.scrollTop !== top || node.scrollLeft !== left)) {
      // A virtualized list of React Aria turns pointer events off on its content while it scrolls,
      // until 300 ms after the last scroll event: a click before that lands on the list, not on what
      // it was aimed at.
      await new Promise((resolve) => setTimeout(resolve, 400));
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    }
    const box = element.getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + Math.min(box.height / 2, 40) };
  })()`);
  if (box === null) {
    throw new Error(`nothing to click at ${selector}`);
  }
  for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
    await cdp.send('Input.dispatchMouseEvent', {
      type,
      x: box.x,
      y: box.y,
      button: type === 'mouseMoved' ? 'none' : 'left',
      clickCount: type === 'mouseMoved' ? 0 : 1,
      modifiers,
    });
  }
}

/** Types into a field: focuses it with a click, selects what is there and writes over it. */
export async function typeInto(cdp, selector, text) {
  await click(cdp, selector);
  // What is there is selected, so what is typed replaces it.
  await cdp.evaluate(`document.querySelector(${JSON.stringify(selector)}).select()`);
  if (text === '') {
    await cdp.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Backspace', code: 'Backspace', windowsVirtualKeyCode: 8 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Backspace', code: 'Backspace', windowsVirtualKeyCode: 8 });
    return;
  }
  await cdp.send('Input.insertText', { text });
}

/** Waits until `expression` is truthy in the page, and returns its value. */
export async function waitFor(cdp, expression, { timeoutMs = 60_000, intervalMs = 100, what = expression } = {}) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    let value;
    try {
      value = await cdp.evaluate(expression);
    } catch {
      value = undefined;
    }
    if (value) {
      return value;
    }
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${what}`);
    }
    await delay(intervalMs);
  }
}

let nextPort = 9360;

/** Starts the app with its debugging port open, and connects to its main page. */
export async function launch(app, args, readyExpression) {
  const port = nextPort++;
  // The WebView2's profile beside the run's library, in the run's own temporary folder: a profile
  // is shared only by instances started with the same options, so a run would not start beside
  // the app a person has open, and must not touch that person's profile anyway.
  const library = args[args.indexOf('--library') + 1];
  const profile = args.includes('--library') && library !== undefined ? { WEBVIEW2_USER_DATA_FOLDER: `${library}-webview2` } : {};
  const child = spawn(app, args, {
    env: {
      ...process.env,
      ...profile,
      // Only for this run: the WebView answers DevTools on this port. The app's own arguments are
      // repeated, with the ones its benchmarks use so that a covered window is not slowed down.
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: [
        `--remote-debugging-port=${port}`,
        '--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion',
        '--disable-background-timer-throttling',
        '--disable-renderer-backgrounding',
        '--disable-backgrounding-occluded-windows',
      ].join(' '),
    },
    stdio: ['ignore', 'ignore', 'pipe'],
  });
  let log = '';
  child.stderr.on('data', (chunk) => {
    log += chunk;
  });
  const exited = new Promise((resolveExit) => child.on('exit', (code) => resolveExit(code)));
  const deadline = Date.now() + 60_000;
  let target;
  while (target === undefined) {
    if (Date.now() > deadline) {
      child.kill();
      throw new Error(`the app never offered a page to debug\n${log}`);
    }
    try {
      const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      target = targets.find((candidate) => candidate.type === 'page' && !candidate.url.startsWith('devtools'));
    } catch {
      // Not listening yet.
    }
    if (target === undefined) {
      await delay(200);
    }
  }
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.connect();
  // The page behaves as if it had the focus, without the window being given it.
  await cdp.send('Emulation.setFocusEmulationEnabled', { enabled: true });
  // What the page and its frames say on their consoles, errors and warnings, for the report.
  const console = [];
  cdp.listeners.add(({ method, params, sessionId }) => {
    if (console.length >= 400) {
      return;
    }
    if (method === 'Runtime.consoleAPICalled' && (params.type === 'error' || params.type === 'warning')) {
      const text = params.args.map((arg) => arg.value ?? arg.description ?? '').join(' ');
      console.push({ type: params.type, frame: sessionId ?? 'page', text: String(text).slice(0, 600) });
    } else if (method === 'Runtime.exceptionThrown') {
      const details = params.exceptionDetails;
      console.push({ type: 'exception', frame: sessionId ?? 'page', text: String(details.exception?.description ?? details.text).slice(0, 600) });
    }
  });
  // Every frame is followed from the start, so none is missed.
  const frames = new Frames(cdp);
  framesOf.set(cdp, frames);
  await frames.start();
  await waitFor(cdp, readyExpression, { timeoutMs: 60_000, what: 'the app to start' });
  return { child, cdp, port, exited, log: () => log, console };
}

/**
 * Follows every JavaScript context of a page and of the frames in it, whether a frame runs in the
 * page's own process or in one of its own, so that code can be run in any frame by its origin.
 */
class Frames {
  constructor(cdp) {
    this.cdp = cdp;
    this.contexts = [];
    cdp.listeners.add((message) => this.onEvent(message));
  }

  async start() {
    await this.cdp.send('Runtime.enable');
    await this.cdp.send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true });
  }

  onEvent({ method, params, sessionId }) {
    if (method === 'Runtime.executionContextCreated') {
      const { context } = params;
      if (context.auxData?.isDefault !== false) {
        this.contexts.push({ id: context.id, origin: context.origin, sessionId });
      }
    } else if (method === 'Runtime.executionContextDestroyed') {
      this.contexts = this.contexts.filter((c) => !(c.id === params.executionContextId && c.sessionId === sessionId));
    } else if (method === 'Runtime.executionContextsCleared') {
      this.contexts = this.contexts.filter((c) => c.sessionId !== sessionId);
    } else if (method === 'Target.attachedToTarget') {
      const child = params.sessionId;
      void this.cdp.send('Runtime.enable', {}, 30_000, child).catch(() => {});
      void this.cdp.send('Runtime.runIfWaitingForDebugger', {}, 30_000, child).catch(() => {});
    } else if (method === 'Target.detachedFromTarget') {
      this.contexts = this.contexts.filter((c) => c.sessionId !== params.sessionId);
    }
  }

  find(origin) {
    for (let at = this.contexts.length - 1; at >= 0; at--) {
      if (this.contexts[at].origin === origin) {
        return this.contexts[at];
      }
    }
    return null;
  }
}

const framesOf = new WeakMap();

/**
 * A handle on the frame of the app whose origin is `origin`, to run code in it. It finds the
 * frame's newest context each time, so it follows the frame through a reload.
 */
export async function connectFrame(app, origin, pathname = '/reader.html', timeoutMs = 30_000) {
  let frames = framesOf.get(app.cdp);
  if (frames === undefined) {
    frames = new Frames(app.cdp);
    framesOf.set(app.cdp, frames);
    await frames.start();
  }
  // Frames made from blob: URLs share the origin of the page that made them, so the origin alone
  // does not tell a page from its own frames: the path does.
  let chosen = null;
  const pick = async () => {
    if (chosen !== null && frames.contexts.includes(chosen)) {
      return chosen;
    }
    chosen = null;
    const candidates = frames.contexts.filter((context) => context.origin === origin).reverse();
    for (const context of candidates) {
      try {
        const path = await app.cdp.evaluate('location.pathname', 5000, {
          contextId: context.id,
          sessionId: context.sessionId,
        });
        if (path === pathname) {
          chosen = context;
          return chosen;
        }
      } catch {
        // Gone in the meantime.
      }
    }
    return null;
  };
  const deadline = Date.now() + timeoutMs;
  while ((await pick()) === null) {
    if (Date.now() > deadline) {
      throw new Error(`no frame at ${origin}${pathname}: ${frames.contexts.map((c) => c.origin).join(', ')}`);
    }
    await delay(100);
  }
  return {
    evaluate: async (expression, evaluateTimeoutMs = 30_000) => {
      const context = await pick();
      if (context === null) {
        throw new Error(`the frame at ${origin}${pathname} is gone`);
      }
      return app.cdp.evaluate(expression, evaluateTimeoutMs, { contextId: context.id, sessionId: context.sessionId });
    },
    close: () => {},
  };
}

/** Closes the window the way its close button would, and waits for the program to end. */
export async function closeApp(app) {
  app.cdp.close();
  execFileSync(
    'powershell',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      `$p = Get-Process -Id ${app.child.pid} -ErrorAction SilentlyContinue; if ($p) { [void]$p.CloseMainWindow() }`,
    ],
    { windowsHide: true },
  );
  const code = await Promise.race([app.exited, delay(20_000).then(() => 'timeout')]);
  if (code === 'timeout') {
    app.child.kill();
    throw new Error('the app did not close when asked');
  }
  return code;
}
