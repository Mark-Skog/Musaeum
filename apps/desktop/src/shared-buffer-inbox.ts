/**
 * Receives the host's WebView2 shared buffers on the main thread, which is the only place
 * they arrive. They carry no metadata, so each one is recognised by the magic number in its
 * first word. Listening starts at construction, before the host is asked to post anything.
 */

export interface WebviewSharedBufferEvent extends Event {
  getBuffer(): ArrayBuffer;
}

export interface WebviewApi {
  addEventListener(
    type: 'sharedbufferreceived',
    listener: (event: WebviewSharedBufferEvent) => void,
  ): void;
}

interface Waiter {
  magic: number;
  resolve: (buffer: ArrayBuffer) => void;
}

export class SharedBufferInbox {
  private readonly arrived: ArrayBuffer[] = [];
  private readonly waiters: Waiter[] = [];

  constructor(webview: WebviewApi) {
    webview.addEventListener('sharedbufferreceived', (event) => this.receive(event.getBuffer()));
  }

  private receive(buffer: ArrayBuffer): void {
    const magic = new Uint32Array(buffer, 0, 1)[0];
    const at = this.waiters.findIndex((waiter) => waiter.magic === magic);
    if (at >= 0) {
      this.waiters.splice(at, 1)[0].resolve(buffer);
    } else {
      this.arrived.push(buffer);
    }
  }

  /** The next buffer with this magic number, waiting at most `timeoutMs` for it to arrive. */
  next(magic: number, timeoutMs: number): Promise<ArrayBuffer> {
    const at = this.arrived.findIndex((buffer) => new Uint32Array(buffer, 0, 1)[0] === magic);
    if (at >= 0) {
      return Promise.resolve(this.arrived.splice(at, 1)[0]);
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiters.splice(this.waiters.indexOf(waiter), 1);
        reject(new Error('the host never posted a shared buffer it promised'));
      }, timeoutMs);
      const waiter: Waiter = {
        magic,
        resolve: (buffer) => {
          clearTimeout(timer);
          resolve(buffer);
        },
      };
      this.waiters.push(waiter);
    });
  }
}
