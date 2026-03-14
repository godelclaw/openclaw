import { describe, expect, it } from "vitest";
import { extractVeriCoreContentFromMessage } from "./bot-handlers.js";

describe("extractVeriCoreContentFromMessage", () => {
  it("includes structured reply context for replied bot messages", () => {
    const body = extractVeriCoreContentFromMessage({
      message_id: 2,
      date: 1100,
      chat: { id: 1, type: "private" },
      text: "Well, look 2-3 messages up. We were talking about moltbook, yes?",
      reply_to_message: {
        message_id: 1,
        date: 1000,
        chat: { id: 1, type: "private" },
        text: "Moltbook is alive and well! Here's what happened.",
        from: { id: 42, first_name: "Oruzi", is_bot: true },
      },
      // oxlint-disable-next-line typescript/no-explicit-any
    } as any);

    expect(body).toContain("Well, look 2-3 messages up. We were talking about moltbook, yes?");
    expect(body).toContain("[Replying to Oruzi id:1]");
    expect(body).toContain("Moltbook is alive and well! Here's what happened.");
    expect(body).not.toContain("[Reply context]");
  });

  it("includes quoted forwarded context when available", () => {
    const body = extractVeriCoreContentFromMessage({
      message_id: 3,
      date: 1200,
      chat: { id: 1, type: "private" },
      text: "This is the part I meant.",
      quote: { text: "Original quoted line" },
      reply_to_message: {
        message_id: 2,
        date: 1100,
        chat: { id: 1, type: "private" },
        text: "Forwarded content",
        from: { id: 7, first_name: "Alice", is_bot: false },
        forward_origin: {
          type: "user",
          sender_user: {
            id: 999,
            first_name: "Bob",
            last_name: "Smith",
            username: "bobsmith",
            is_bot: false,
          },
          date: 500,
        },
      },
      // oxlint-disable-next-line typescript/no-explicit-any
    } as any);

    expect(body).toContain("[Quoting Alice id:2]");
    expect(body).toContain("[Forwarded from Bob Smith (@bobsmith) at 1970-01-01T00:08:20.000Z]");
    expect(body).toContain('"Original quoted line"');
  });
});
