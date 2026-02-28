import { describe, expect, it } from "vitest";

import type { FinalizedMsgContext } from "../auto-reply/templating.js";
import { buildVeriCoreStimulusInput, deriveVeriCoreChannel } from "./impetus.js";

function makeCtx(overrides: Partial<FinalizedMsgContext> = {}): FinalizedMsgContext {
  return {
    CommandAuthorized: false,
    ...overrides,
  };
}

describe("vericore impetus bridge helpers", () => {
  it("maps telegram DM to telegram_dm", () => {
    const channel = deriveVeriCoreChannel(
      makeCtx({
        Surface: "telegram",
        ChatType: "private",
      }),
    );

    expect(channel).toBe("telegram_dm");
  });

  it("maps telegram group to telegram_public", () => {
    const channel = deriveVeriCoreChannel(
      makeCtx({
        Surface: "telegram",
        ChatType: "supergroup",
      }),
    );

    expect(channel).toBe("telegram_public");
  });

  it("maps family-routed telegram sessions to telegram_family", () => {
    const channel = deriveVeriCoreChannel(
      makeCtx({
        Surface: "telegram",
        ChatType: "supergroup",
        SessionKey: "agent:family:telegram:group:1234",
      }),
    );

    expect(channel).toBe("telegram_family");
  });

  it("maps heartbeat events to internal", () => {
    const channel = deriveVeriCoreChannel(
      makeCtx({
        Provider: "heartbeat",
      }),
    );

    expect(channel).toBe("internal");
  });

  it("builds stimulus payload from context", () => {
    const stimulus = buildVeriCoreStimulusInput(
      makeCtx({
        Surface: "telegram",
        ChatType: "private",
        SenderId: "u123",
        BodyForCommands: "/status",
        Timestamp: 1739960000,
      }),
    );

    expect(stimulus).toEqual({
      channel: "telegram_dm",
      actor: "u123",
      content: "/status",
      timestamp: 1739960000,
    });
  });
});
