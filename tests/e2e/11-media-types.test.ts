/**
 * Test 11: Media and content type handling.
 *
 * Tests that all supported message types (image, file, video, audio,
 * location, notice, emote) are correctly bridged from external → Matrix.
 * Uses portal auto-creation so no pre-existing room mapping is needed.
 */
import { describe, test, expect } from "bun:test";
import { env } from "./env";
import { bridgeSendContent, bridgeUploadMedia } from "./helpers";

const platform = `test_media_${Date.now()}`;
const extRoom = `ext_media_${Date.now()}`;

describe("Content Types", () => {
  test("text message", async () => {
    const resp = await bridgeSendContent({
      platform,
      roomId: extRoom,
      senderId: "media_user",
      content: { type: "text", body: "plain text", html: "<b>bold</b>" },
    });
    expect(resp.ok).toBe(true);
  });

  test("image message", async () => {
    const resp = await bridgeSendContent({
      platform,
      roomId: extRoom,
      senderId: "media_user",
      content: {
        type: "image",
        url: "mxc://example.com/fake_image",
        caption: "A test image",
        mimetype: "image/jpeg",
      },
    });
    expect(resp.ok).toBe(true);
  });

  test("file message", async () => {
    const resp = await bridgeSendContent({
      platform,
      roomId: extRoom,
      senderId: "media_user",
      content: {
        type: "file",
        url: "mxc://example.com/fake_file",
        filename: "document.pdf",
        mimetype: "application/pdf",
      },
    });
    expect(resp.ok).toBe(true);
  });

  test("video message", async () => {
    const resp = await bridgeSendContent({
      platform,
      roomId: extRoom,
      senderId: "media_user",
      content: {
        type: "video",
        url: "mxc://example.com/fake_video",
        caption: "A test video",
        mimetype: "video/mp4",
      },
    });
    expect(resp.ok).toBe(true);
  });

  test("audio message", async () => {
    const resp = await bridgeSendContent({
      platform,
      roomId: extRoom,
      senderId: "media_user",
      content: {
        type: "audio",
        url: "mxc://example.com/fake_audio",
        mimetype: "audio/ogg",
      },
    });
    expect(resp.ok).toBe(true);
  });

  test("location message", async () => {
    const resp = await bridgeSendContent({
      platform,
      roomId: extRoom,
      senderId: "media_user",
      content: {
        type: "location",
        latitude: 37.7749,
        longitude: -122.4194,
      },
    });
    expect(resp.ok).toBe(true);
  });

  test("notice message", async () => {
    const resp = await bridgeSendContent({
      platform,
      roomId: extRoom,
      senderId: "media_user",
      content: { type: "notice", body: "This is a notice" },
    });
    expect(resp.ok).toBe(true);
  });

  test("emote message", async () => {
    const resp = await bridgeSendContent({
      platform,
      roomId: extRoom,
      senderId: "media_user",
      content: { type: "emote", body: "waves hello" },
    });
    expect(resp.ok).toBe(true);
  });

  test("image with default mimetype", async () => {
    const resp = await bridgeSendContent({
      platform,
      roomId: extRoom,
      senderId: "media_user",
      content: {
        type: "image",
        url: "mxc://example.com/default_mime",
      },
    });
    expect(resp.ok).toBe(true);
  });
});

describe("Media Upload", () => {
  test("upload returns mxc:// URI", async () => {
    const data = new TextEncoder().encode("fake image data for testing");
    const result = await bridgeUploadMedia(data, "test.png", "image/png");

    expect(result.content_uri).toMatch(/^mxc:\/\//);
    expect(result.filename).toBe("test.png");
    expect(result.size).toBe(data.length);
  });

  test("upload with no file returns 400", async () => {
    const resp = await fetch(`${env.bridgeUrl}/api/v1/upload`, {
      method: "POST",
      body: new FormData(),
    });
    expect(resp.status).toBe(400);
  });
});
