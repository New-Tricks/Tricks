// JSON-RPC 2.0 over stdio with LSP-style Content-Length framing. No VS Code imports,
// so it can be unit-tested with plain Node.

export interface RpcError {
  code: number;
  message: string;
  data?: { prompt?: string; details?: string[] };
}

export const CONFIRMATION_REQUIRED = -32001;

export class MessageReader {
  private buf = Buffer.alloc(0);
  constructor(private readonly onMessage: (msg: any) => void) {}

  push(chunk: Buffer): void {
    this.buf = Buffer.concat([this.buf, chunk]);
    for (;;) {
      const headerEnd = this.buf.indexOf("\r\n\r\n");
      if (headerEnd < 0) return;
      const header = this.buf.subarray(0, headerEnd).toString("ascii");
      const m = /Content-Length:\s*(\d+)/i.exec(header);
      if (!m) {
        this.buf = this.buf.subarray(headerEnd + 4);
        continue;
      }
      const len = parseInt(m[1], 10);
      const start = headerEnd + 4;
      if (this.buf.length < start + len) return;
      const body = this.buf.subarray(start, start + len).toString("utf8");
      this.buf = this.buf.subarray(start + len);
      try {
        this.onMessage(JSON.parse(body));
      } catch {
        // ignore malformed message
      }
    }
  }
}

export function encode(msg: unknown): Buffer {
  const body = Buffer.from(JSON.stringify(msg), "utf8");
  return Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, "ascii"), body]);
}

export class RpcFailure extends Error {
  constructor(public readonly error: RpcError) {
    super(error.message);
  }
  get needsConfirmation(): boolean {
    return this.error.code === CONFIRMATION_REQUIRED;
  }
}
