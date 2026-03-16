/**
 * Bridge API client — wraps HTTP calls to the Matrix Bridge REST API.
 */

import { config } from "./config.js";

interface BridgeRequestOptions {
  method: string;
  path: string;
  body?: unknown;
}

async function bridgeRequest<T = unknown>({
  method,
  path,
  body,
}: BridgeRequestOptions): Promise<T> {
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
  };
  if (config.bridgeApiKey) {
    headers["Authorization"] = `Bearer ${config.bridgeApiKey}`;
  }

  const resp = await fetch(`${config.bridgeUrl}${path}`, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
  });

  const text = await resp.text();
  if (!resp.ok) {
    throw new Error(`Bridge API ${method} ${path} failed (${resp.status}): ${text}`);
  }

  return text ? JSON.parse(text) : ({} as T);
}

// ─── Room mappings ───────────────────────────────────────────────────────────

export interface CreateRoomResult {
  id: number;
  matrix_room_id: string;
}

export async function createRoomMapping(
  platform: string,
  externalRoomId: string,
  matrixRoomId?: string,
): Promise<CreateRoomResult> {
  const body: Record<string, string> = { platform, external_room_id: externalRoomId };
  if (matrixRoomId) body.matrix_room_id = matrixRoomId;
  return bridgeRequest({ method: "POST", path: "/api/v1/rooms", body });
}

export async function deleteRoomMapping(id: number): Promise<void> {
  await bridgeRequest({ method: "DELETE", path: `/api/v1/rooms/${id}` });
}

// ─── Webhooks ────────────────────────────────────────────────────────────────

export interface CreateWebhookResult {
  id: number;
}

export async function createWebhook(
  platform: string,
  url: string,
  forwardSources: string[] = ["*"],
): Promise<CreateWebhookResult> {
  return bridgeRequest({
    method: "POST",
    path: "/api/v1/webhooks",
    body: { platform, url, forward_sources: forwardSources },
  });
}

export async function deleteWebhook(id: number): Promise<void> {
  await bridgeRequest({ method: "DELETE", path: `/api/v1/webhooks/${id}` });
}

// ─── Messages ────────────────────────────────────────────────────────────────

export interface SendMessageResult {
  event_id: string;
  message_id: string;
}

export interface MessageContent {
  type: string;
  [key: string]: unknown;
}

export async function sendMessage(
  platform: string,
  roomId: string,
  senderId: string,
  senderName: string,
  content: MessageContent,
  externalMessageId?: string,
): Promise<SendMessageResult> {
  const body: Record<string, unknown> = {
    platform,
    room_id: roomId,
    sender: { id: senderId, display_name: senderName },
    content,
  };
  if (externalMessageId) body.external_message_id = externalMessageId;
  return bridgeRequest({ method: "POST", path: "/api/v1/message", body });
}

// ─── Upload ──────────────────────────────────────────────────────────────────

export interface UploadResult {
  content_uri: string;
  filename: string;
  size: number;
}

export async function uploadFile(
  fileData: Buffer,
  filename: string,
  contentType: string,
): Promise<UploadResult> {
  const formData = new FormData();
  const blob = new Blob([fileData], { type: contentType });
  formData.append("file", blob, filename);

  const headers: Record<string, string> = {};
  if (config.bridgeApiKey) {
    headers["Authorization"] = `Bearer ${config.bridgeApiKey}`;
  }

  const resp = await fetch(`${config.bridgeUrl}/api/v1/upload`, {
    method: "POST",
    headers,
    body: formData,
  });

  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`Upload failed (${resp.status}): ${text}`);
  }

  return resp.json();
}

// ─── Admin ───────────────────────────────────────────────────────────────────

export async function getServerInfo(): Promise<Record<string, unknown>> {
  return bridgeRequest({ method: "GET", path: "/api/v1/admin/info" });
}

export async function getCryptoStatus(): Promise<Record<string, unknown>> {
  return bridgeRequest({ method: "GET", path: "/api/v1/admin/crypto" });
}
