/**
 * Temporary webhook server — receives outbound messages from the bridge
 * during integration tests. Collects payloads for later assertion.
 */

import * as http from "node:http";

export interface WebhookPayload {
  event: string;
  platform: string;
  source_platform?: string;
  message: {
    id: string;
    sender: {
      platform: string;
      external_id: string;
      display_name: string | null;
      avatar_url: string | null;
    };
    room: {
      platform: string;
      external_id: string;
    };
    content: Record<string, unknown>;
    timestamp: number;
    reply_to: string | null;
  };
}

export class WebhookServer {
  public payloads: WebhookPayload[] = [];
  private server: http.Server;
  private port: number;

  constructor(port = 0) {
    this.port = port;
    this.server = http.createServer(async (req, res) => {
      if (req.method === "POST" && req.url === "/webhook") {
        const chunks: Buffer[] = [];
        for await (const chunk of req) chunks.push(chunk as Buffer);
        const body = Buffer.concat(chunks).toString("utf-8");

        try {
          const payload = JSON.parse(body) as WebhookPayload;
          this.payloads.push(payload);
        } catch (err) {
          console.warn("  [webhook] failed to parse payload:", err);
        }

        res.writeHead(200, { "Content-Type": "application/json" });
        res.end('{"status":"ok"}');
      } else {
        res.writeHead(404);
        res.end("Not Found");
      }
    });
  }

  /** Start listening. Returns the actual port (useful when port=0). */
  async start(): Promise<number> {
    return new Promise((resolve) => {
      this.server.listen(this.port, "0.0.0.0", () => {
        const addr = this.server.address();
        if (typeof addr === "object" && addr) {
          this.port = addr.port;
        }
        console.log(`  [webhook] server listening on port ${this.port}`);
        resolve(this.port);
      });
    });
  }

  /** Stop the server. */
  async stop(): Promise<void> {
    return new Promise((resolve) => {
      this.server.close(() => {
        console.log("  [webhook] server stopped");
        resolve();
      });
    });
  }

  /** Get the webhook URL for this server. */
  get url(): string {
    return `http://localhost:${this.port}/webhook`;
  }

  /** Clear collected payloads. */
  clear(): void {
    this.payloads.length = 0;
  }

  /**
   * Wait for a payload matching the predicate.
   * Returns the matching payload or throws on timeout.
   */
  async waitForPayload(
    predicate: (p: WebhookPayload) => boolean,
    timeoutMs = 15000,
  ): Promise<WebhookPayload> {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
      const found = this.payloads.find(predicate);
      if (found) return found;
      await new Promise((r) => setTimeout(r, 300));
    }
    throw new Error(`Timed out waiting for webhook payload (${timeoutMs}ms)`);
  }
}
