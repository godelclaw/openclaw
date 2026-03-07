import type { OpenClawConfig } from "../config/config.js";
import { routeReply } from "../auto-reply/reply/route-reply.js";
import { createSubsystemLogger } from "../logging/subsystem.js";

const log = createSubsystemLogger("vericore/operator-alert");

export function resolveOperatorTelegramChatId(cfg: OpenClawConfig): string | undefined {
  const tg = cfg.channels?.telegram;
  if (tg?.defaultTo != null) return String(tg.defaultTo);
  const first = tg?.allowFrom?.find((id) => /^\d+$/.test(String(id)));
  return first != null ? String(first) : undefined;
}

export async function sendBridgeFallbackAlert(params: {
  cfg: OpenClawConfig;
  alertMsg: string;
  operatorChatId: string;
}): Promise<void> {
  try {
    const result = await routeReply({
      payload: { text: params.alertMsg },
      channel: "telegram",
      to: params.operatorChatId,
      cfg: params.cfg,
      mirror: false,
    });
    if (!result.ok) {
      log.warn(`fallback alert delivery failed: ${result.error ?? "unknown"}`);
    }
  } catch (err) {
    log.warn(`fallback alert delivery error: ${String(err)}`);
  }
}
