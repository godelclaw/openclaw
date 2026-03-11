import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { OpenClawConfig } from "../config/config.js";
import {
  CANONICAL_HEARTBEAT_PROMPT,
  resolveHeartbeatSyncObservedInput,
  syncHeartbeatArtifacts,
} from "./heartbeat-sync.js";

describe("heartbeat sync", () => {
  const tempRoots: string[] = [];

  afterEach(async () => {
    while (tempRoots.length > 0) {
      const root = tempRoots.pop();
      if (root) {
        await fs.rm(root, { recursive: true, force: true });
      }
    }
  });

  async function makeRoot(): Promise<string> {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), "heartbeat-sync-"));
    tempRoots.push(root);
    return root;
  }

  it("returns null when no files exist and no explicit prompt is configured", async () => {
    const root = await makeRoot();
    const observed = await resolveHeartbeatSyncObservedInput({
      cfg: {},
      workspaceDir: root,
      stateDir: path.join(root, "state"),
    });
    expect(observed).toBeNull();
  });

  it("observes matching canonical and mirrored heartbeat files", async () => {
    const root = await makeRoot();
    const workspaceDir = path.join(root, "workspace");
    const stateDir = path.join(root, "state");
    await fs.mkdir(workspaceDir, { recursive: true });
    await fs.mkdir(path.join(stateDir, "workspace"), { recursive: true });
    const content = "On heartbeat:\\n1. Be calm.\\n";
    await fs.writeFile(path.join(workspaceDir, "HEARTBEAT.md"), content, "utf-8");
    await fs.writeFile(path.join(stateDir, "workspace", "HEARTBEAT.md"), content, "utf-8");

    const observed = await resolveHeartbeatSyncObservedInput({
      cfg: {
        agents: { defaults: { heartbeat: { prompt: CANONICAL_HEARTBEAT_PROMPT } } },
      },
      workspaceDir,
      stateDir,
    });

    expect(observed).toEqual({
      canonical_available: true,
      mirror_available: true,
      mirror_matches: true,
      prompt_uses_file_reference: true,
      prompt_mentions_legacy_gas: false,
    });
  });

  it("syncs the mirrored file and repairs a legacy GAS prompt", async () => {
    const root = await makeRoot();
    const workspaceDir = path.join(root, "workspace");
    const stateDir = path.join(root, "state");
    await fs.mkdir(workspaceDir, { recursive: true });
    const canonicalContent = "On heartbeat:\\n1. Check mailbox.\\n";
    await fs.writeFile(path.join(workspaceDir, "HEARTBEAT.md"), canonicalContent, "utf-8");

    const cfg: OpenClawConfig = {
      agents: {
        defaults: {
          heartbeat: {
            prompt:
              "Heartbeat. Check GAS.md for your budget status. If you have something meaningful to share or do, go ahead. Otherwise reply HEARTBEAT_OK.",
          },
        },
      },
    };
    const writeConfig = vi.fn(async () => {});

    const result = await syncHeartbeatArtifacts({
      cfg,
      workspaceDir,
      stateDir,
      writeConfig,
    });

    expect(result.canonicalAvailable).toBe(true);
    expect(result.mirrorUpdated).toBe(true);
    expect(result.promptUpdated).toBe(true);
    await expect(fs.readFile(path.join(stateDir, "workspace", "HEARTBEAT.md"), "utf-8")).resolves
      .toBe(canonicalContent);
    expect(cfg.agents?.defaults?.heartbeat?.prompt).toBe(CANONICAL_HEARTBEAT_PROMPT);
    expect(writeConfig).toHaveBeenCalledTimes(1);
  });
});
