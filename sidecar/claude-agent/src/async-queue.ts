// AsyncPromptQueue — unbounded async queue that implements
// `AsyncIterable<T>` so it can be handed to the SDK's `query()` as a
// streaming prompt source.
//
// Semantics:
//   * `push(msg)` enqueues and wakes whichever async reader is parked.
//   * `close()` signals EOF. After the queue drains, the iterator
//     terminates cleanly.
//   * Only one iterator at a time is supported — the SDK will only
//     ever request one. Trying to iterate twice throws.

/** Unbounded FIFO async queue backed by a linked-list of pending items
 *  and a single parked reader (when the queue is empty). */
export class AsyncPromptQueue<T> implements AsyncIterable<T> {
  private queue: T[] = [];
  private closed = false;
  private resolveNext: ((value: IteratorResult<T>) => void) | null = null;
  private iteratorStarted = false;

  /** Enqueue one item. Wakes a parked reader if present. */
  push(value: T): void {
    if (this.closed) {
      throw new Error("AsyncPromptQueue is closed");
    }
    if (this.resolveNext) {
      const resolve = this.resolveNext;
      this.resolveNext = null;
      resolve({ value, done: false });
      return;
    }
    this.queue.push(value);
  }

  /** Signal end-of-stream. After `close()` any pending reader receives
   *  `{done: true}`; further `push()` throws. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    if (this.resolveNext) {
      const resolve = this.resolveNext;
      this.resolveNext = null;
      resolve({ value: undefined as unknown as T, done: true });
    }
  }

  /** Whether [`close`](#close) has been called. */
  isClosed(): boolean {
    return this.closed;
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    if (this.iteratorStarted) {
      throw new Error("AsyncPromptQueue can only be iterated once");
    }
    this.iteratorStarted = true;
    return {
      next: (): Promise<IteratorResult<T>> => {
        if (this.queue.length > 0) {
          const value = this.queue.shift() as T;
          return Promise.resolve({ value, done: false });
        }
        if (this.closed) {
          return Promise.resolve({
            value: undefined as unknown as T,
            done: true,
          });
        }
        return new Promise<IteratorResult<T>>((resolve) => {
          this.resolveNext = resolve;
        });
      },
      return: (): Promise<IteratorResult<T>> => {
        this.close();
        return Promise.resolve({
          value: undefined as unknown as T,
          done: true,
        });
      },
    };
  }
}
