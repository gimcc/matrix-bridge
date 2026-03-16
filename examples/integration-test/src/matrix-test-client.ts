/**
 * Matrix test client — wraps matrix-bot-sdk with E2EE support.
 *
 * Provides helpers for:
 * - Joining/creating encrypted rooms
 * - Sending plain and encrypted text/media messages
 * - Collecting incoming messages for assertion
 * - Encrypting and decrypting file attachments
 */

import * as fs from "node:fs";
import * as path from "node:path";
import * as crypto from "node:crypto";
import { deflateSync } from "node:zlib";
import {
  MatrixClient,
  SimpleFsStorageProvider,
  RustSdkCryptoStorageProvider,
  AutojoinRoomsMixin,
} from "matrix-bot-sdk";
import { config } from "./config.js";

// ─── Types ───────────────────────────────────────────────────────────────────

export interface ReceivedMessage {
  roomId: string;
  eventId: string;
  sender: string;
  type: string;
  content: Record<string, unknown>;
  receivedAt: number;
}

export interface EncryptedFileInfo {
  url: string;
  key: {
    kty: string;
    key_ops: string[];
    alg: string;
    k: string;
    ext: boolean;
  };
  iv: string;
  hashes: { sha256: string };
  v: string;
}

// ─── Client ──────────────────────────────────────────────────────────────────

export class MatrixTestClient {
  public client: MatrixClient;
  public messages: ReceivedMessage[] = [];
  private started = false;

  constructor() {
    // Ensure storage directories exist.
    const storageDir = path.dirname(config.storageFile);
    if (storageDir !== "." && !fs.existsSync(storageDir)) {
      fs.mkdirSync(storageDir, { recursive: true });
    }
    if (!fs.existsSync(config.cryptoDir)) {
      fs.mkdirSync(config.cryptoDir, { recursive: true });
    }

    const storage = new SimpleFsStorageProvider(config.storageFile);
    const cryptoProvider = new RustSdkCryptoStorageProvider(config.cryptoDir);

    this.client = new MatrixClient(
      config.homeserverUrl,
      config.botAccessToken,
      storage,
      cryptoProvider,
    );

    // Auto-accept invites so the test bot joins rooms automatically.
    AutojoinRoomsMixin.setupOnClient(this.client);
  }

  /** Start the client (initializes crypto, begins syncing). */
  async start(): Promise<void> {
    if (this.started) return;

    // Collect all incoming messages (including own messages for roundtrip tests).
    this.client.on("room.message", (roomId: string, event: Record<string, unknown>) => {
      const content = (event.content ?? {}) as Record<string, unknown>;
      if (!content.msgtype) return;

      this.messages.push({
        roomId,
        eventId: (event.event_id as string) ?? "",
        sender: (event.sender as string) ?? "",
        type: (content.msgtype as string) ?? "",
        content,
        receivedAt: Date.now(),
      });
    });

    // Also listen for failed decryptions for diagnostics.
    this.client.on(
      "room.failed_decryption",
      (roomId: string, event: unknown, error: Error) => {
        console.warn(`  [crypto] failed to decrypt in ${roomId}: ${error.message}`);
      },
    );

    await this.client.start();
    this.started = true;
    console.log("  [matrix] client started with E2EE");
  }

  /** Stop the client gracefully. */
  async stop(): Promise<void> {
    if (!this.started) return;
    this.client.stop();
    this.started = false;
    console.log("  [matrix] client stopped");
  }

  /** Clear collected messages. */
  clearMessages(): void {
    this.messages.length = 0;
  }

  /**
   * Wait until a message matching the predicate arrives.
   * Returns the matching message or throws on timeout.
   */
  async waitForMessage(
    predicate: (msg: ReceivedMessage) => boolean,
    timeoutMs = 15000,
  ): Promise<ReceivedMessage> {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
      const found = this.messages.find(predicate);
      if (found) return found;
      await new Promise((r) => setTimeout(r, 300));
    }
    throw new Error(`Timed out waiting for message (${timeoutMs}ms)`);
  }

  // ─── Room management ────────────────────────────────────────────────────

  /** Create an encrypted room and return its ID. */
  async createEncryptedRoom(name: string): Promise<string> {
    const roomId = await this.client.createRoom({
      name,
      initial_state: [
        {
          type: "m.room.encryption",
          state_key: "",
          content: { algorithm: "m.megolm.v1.aes-sha2" },
        },
      ],
      preset: "private_chat" as any,
    });
    console.log(`  [matrix] created encrypted room: ${roomId}`);
    return roomId;
  }

  /** Create an unencrypted room and return its ID. */
  async createPlainRoom(name: string): Promise<string> {
    const roomId = await this.client.createRoom({
      name,
      preset: "private_chat" as any,
    });
    console.log(`  [matrix] created plain room: ${roomId}`);
    return roomId;
  }

  /** Invite another user to a room. */
  async inviteUser(roomId: string, userId: string): Promise<void> {
    await this.client.inviteUser(userId, roomId);
  }

  /** Check if a room is encrypted. */
  async isRoomEncrypted(roomId: string): Promise<boolean> {
    try {
      return await this.client.crypto.isRoomEncrypted(roomId);
    } catch {
      return false;
    }
  }

  // ─── Text messages ──────────────────────────────────────────────────────

  /** Send a text message (auto-encrypted if room is encrypted). */
  async sendText(roomId: string, body: string): Promise<string> {
    return this.client.sendMessage(roomId, {
      msgtype: "m.text",
      body,
    });
  }

  /** Send a notice message. */
  async sendNotice(roomId: string, body: string): Promise<string> {
    return this.client.sendMessage(roomId, {
      msgtype: "m.notice",
      body,
    });
  }

  /** Send an emote message. */
  async sendEmote(roomId: string, body: string): Promise<string> {
    return this.client.sendMessage(roomId, {
      msgtype: "m.emote",
      body,
    });
  }

  /** Send a text message with HTML formatting. */
  async sendHtml(roomId: string, body: string, html: string): Promise<string> {
    return this.client.sendMessage(roomId, {
      msgtype: "m.text",
      body,
      format: "org.matrix.custom.html",
      formatted_body: html,
    });
  }

  // ─── Plain file upload (unencrypted rooms) ──────────────────────────────

  /** Upload a file and return its mxc:// URI. */
  async uploadFile(data: Buffer, contentType: string, filename: string): Promise<string> {
    return this.client.uploadContent(data, contentType, filename);
  }

  /** Send an image message (plain, not encrypted attachment). */
  async sendImage(
    roomId: string,
    mxcUrl: string,
    filename: string,
    mimetype: string,
    size: number,
  ): Promise<string> {
    return this.client.sendMessage(roomId, {
      msgtype: "m.image",
      body: filename,
      url: mxcUrl,
      info: { mimetype, size },
    });
  }

  /** Send a file message (plain, not encrypted attachment). */
  async sendFile(
    roomId: string,
    mxcUrl: string,
    filename: string,
    mimetype: string,
    size: number,
  ): Promise<string> {
    return this.client.sendMessage(roomId, {
      msgtype: "m.file",
      body: filename,
      url: mxcUrl,
      info: { mimetype, size },
    });
  }

  /** Send a video message (plain). */
  async sendVideo(
    roomId: string,
    mxcUrl: string,
    filename: string,
    mimetype: string,
  ): Promise<string> {
    return this.client.sendMessage(roomId, {
      msgtype: "m.video",
      body: filename,
      url: mxcUrl,
      info: { mimetype },
    });
  }

  /** Send an audio message (plain). */
  async sendAudio(
    roomId: string,
    mxcUrl: string,
    filename: string,
    mimetype: string,
  ): Promise<string> {
    return this.client.sendMessage(roomId, {
      msgtype: "m.audio",
      body: filename,
      url: mxcUrl,
      info: { mimetype },
    });
  }

  // ─── Encrypted file upload (E2EE rooms) ─────────────────────────────────

  /**
   * Encrypt a file, upload the ciphertext, and send it as an encrypted
   * attachment in the given room.
   *
   * This follows the Matrix spec for encrypted attachments:
   * 1. Encrypt with AES-256-CTR via the SDK
   * 2. Upload ciphertext as application/octet-stream
   * 3. Send message with `file` object (key, iv, hash) instead of `url`
   */
  async sendEncryptedImage(
    roomId: string,
    fileData: Buffer,
    filename: string,
    mimetype: string,
  ): Promise<string> {
    const { buffer, file } = await this.client.crypto.encryptMedia(fileData);
    const mxcUrl = await this.client.uploadContent(buffer, "application/octet-stream", filename);

    return this.client.sendMessage(roomId, {
      msgtype: "m.image",
      body: filename,
      file: { ...file, url: mxcUrl },
      info: { mimetype, size: fileData.length },
    });
  }

  /** Send an encrypted file attachment. */
  async sendEncryptedFile(
    roomId: string,
    fileData: Buffer,
    filename: string,
    mimetype: string,
  ): Promise<string> {
    const { buffer, file } = await this.client.crypto.encryptMedia(fileData);
    const mxcUrl = await this.client.uploadContent(buffer, "application/octet-stream", filename);

    return this.client.sendMessage(roomId, {
      msgtype: "m.file",
      body: filename,
      file: { ...file, url: mxcUrl },
      info: { mimetype, size: fileData.length },
    });
  }

  /** Send an encrypted video attachment. */
  async sendEncryptedVideo(
    roomId: string,
    fileData: Buffer,
    filename: string,
    mimetype: string,
  ): Promise<string> {
    const { buffer, file } = await this.client.crypto.encryptMedia(fileData);
    const mxcUrl = await this.client.uploadContent(buffer, "application/octet-stream", filename);

    return this.client.sendMessage(roomId, {
      msgtype: "m.video",
      body: filename,
      file: { ...file, url: mxcUrl },
      info: { mimetype, size: fileData.length },
    });
  }

  /** Send an encrypted audio attachment. */
  async sendEncryptedAudio(
    roomId: string,
    fileData: Buffer,
    filename: string,
    mimetype: string,
  ): Promise<string> {
    const { buffer, file } = await this.client.crypto.encryptMedia(fileData);
    const mxcUrl = await this.client.uploadContent(buffer, "application/octet-stream", filename);

    return this.client.sendMessage(roomId, {
      msgtype: "m.audio",
      body: filename,
      file: { ...file, url: mxcUrl },
      info: { mimetype, size: fileData.length },
    });
  }

  // ─── Encrypted file download & decryption ───────────────────────────────

  /**
   * Download and decrypt an encrypted file from a received message.
   * The message content must have a `file` field (EncryptedFile).
   */
  async downloadAndDecryptFile(
    content: Record<string, unknown>,
  ): Promise<Buffer> {
    const fileInfo = content.file as EncryptedFileInfo | undefined;
    if (!fileInfo?.url) {
      throw new Error("Message content has no encrypted file info");
    }

    // Synapse 1.149+ requires authenticated media download via
    // /_matrix/client/v1/media/download/ instead of the deprecated
    // /_matrix/media/v3/download/.
    // Download the ciphertext manually with auth, then decrypt.
    const mxcMatch = fileInfo.url.match(/^mxc:\/\/([^/]+)\/(.+)$/);
    if (!mxcMatch) {
      throw new Error(`Invalid mxc URL: ${fileInfo.url}`);
    }
    const [, serverName, mediaId] = mxcMatch;
    const downloadUrl =
      `${config.homeserverUrl}/_matrix/client/v1/media/download/${serverName}/${mediaId}`;

    const resp = await fetch(downloadUrl, {
      headers: { Authorization: `Bearer ${config.botAccessToken}` },
    });
    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(`Media download failed (${resp.status}): ${text}`);
    }
    const ciphertext = Buffer.from(await resp.arrayBuffer());

    // Decrypt with AES-256-CTR per Matrix spec.
    const keyData = Buffer.from(
      fileInfo.key.k.replace(/-/g, "+").replace(/_/g, "/"),
      "base64",
    );
    const iv = Buffer.from(fileInfo.iv, "base64");
    // Only first 8 bytes of IV are used (counter starts at 0 for the remaining 8).
    const ivFull = Buffer.alloc(16);
    iv.copy(ivFull, 0, 0, Math.min(iv.length, 16));

    const decipher = crypto.createDecipheriv("aes-256-ctr", keyData, ivFull);
    const decrypted = Buffer.concat([decipher.update(ciphertext), decipher.final()]);
    return decrypted;
  }

  // ─── Location ───────────────────────────────────────────────────────────

  /** Send a location message. */
  async sendLocation(
    roomId: string,
    latitude: number,
    longitude: number,
  ): Promise<string> {
    return this.client.sendMessage(roomId, {
      msgtype: "m.location",
      body: `Location: ${latitude}, ${longitude}`,
      geo_uri: `geo:${latitude},${longitude}`,
    });
  }
}

// ─── Test data generators ────────────────────────────────────────────────────

/** Generate a random PNG image (1x1 pixel with random color). */
export function generateTestImage(): Buffer {
  // Minimal valid PNG: 1x1 pixel, RGBA
  const header = Buffer.from([
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // PNG signature
  ]);
  // IHDR chunk
  const ihdr = Buffer.alloc(25);
  ihdr.writeUInt32BE(13, 0); // length
  ihdr.write("IHDR", 4);
  ihdr.writeUInt32BE(1, 8);  // width
  ihdr.writeUInt32BE(1, 12); // height
  ihdr[16] = 8;  // bit depth
  ihdr[17] = 2;  // color type (RGB)
  ihdr[18] = 0;  // compression
  ihdr[19] = 0;  // filter
  ihdr[20] = 0;  // interlace
  const ihdrCrc = crc32(ihdr.subarray(4, 21));
  ihdr.writeUInt32BE(ihdrCrc, 21);

  // IDAT chunk (zlib-compressed single pixel)
  // Filter byte (0) + R G B
  const r = crypto.randomInt(0, 256);
  const g = crypto.randomInt(0, 256);
  const b = crypto.randomInt(0, 256);
  const rawData = Buffer.from([0x00, r, g, b]);
  const compressed = deflateSync(rawData);
  const idatPayload = Buffer.concat([Buffer.from("IDAT"), compressed]);
  const idatLen = Buffer.alloc(4);
  idatLen.writeUInt32BE(compressed.length);
  const idatCrc = Buffer.alloc(4);
  idatCrc.writeUInt32BE(crc32(idatPayload));

  // IEND chunk
  const iend = Buffer.from([
    0x00, 0x00, 0x00, 0x00,
    0x49, 0x45, 0x4e, 0x44,
    0xae, 0x42, 0x60, 0x82,
  ]);

  return Buffer.concat([header, ihdr, idatLen, idatPayload, idatCrc, iend]);
}

/** Generate a random binary file of the given size. */
export function generateTestFile(size: number): Buffer {
  return crypto.randomBytes(size);
}

/** Generate a simple text file. */
export function generateTextFile(content: string): Buffer {
  return Buffer.from(content, "utf-8");
}

// CRC-32 for PNG chunks (standard polynomial).
const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let c = i;
    for (let j = 0; j < 8; j++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[i] = c;
  }
  return table;
})();

function crc32(buf: Buffer): number {
  let crc = 0xffffffff;
  for (const byte of buf) {
    crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}
