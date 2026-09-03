// @vitest-environment node

import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "./client";

afterEach(() => vi.unstubAllGlobals());

const client = new ApiClient("https://api.example.test");
const list = {
  slack: () => client.listSlackInstallations("workspace-1"),
  lark: () => client.listLarkInstallations("workspace-1"),
  dingtalk: () => client.listDingTalkInstallations("workspace-1"),
  wecom: () => client.listWecomInstallations("workspace-1"),
  telegram: () => client.listTelegramInstallations("workspace-1"),
  weixin: () => client.listWeixinInstallations("workspace-1"),
};

function respond(runtime: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          configured: true,
          installations: [{ id: "installation-1", status: "active", runtime }],
        }),
        { headers: { "Content-Type": "application/json" } },
      ),
    ),
  );
}

describe.each(Object.entries(list))(
  "%s connection status boundary",
  (_name, load) => {
    it("preserves unfamiliar observed states without claiming a known state", async () => {
      const runtime = {
        state: "future_state",
        observedAt: "2026-09-03T10:00:00Z",
        errorCode: null,
      };
      respond(runtime);
      expect((await load()).installations[0]).toMatchObject({
        id: "installation-1",
        runtime,
      });
    });

    it("does not trust a malformed status object from an otherwise valid installation", async () => {
      respond({ state: true, observedAt: 42, errorCode: ["invalid"] });
      const installation = (await load()).installations[0];
      expect(installation).toMatchObject({ id: "installation-1" });
      expect(installation).not.toHaveProperty("runtime.state", true);
    });
  },
);
