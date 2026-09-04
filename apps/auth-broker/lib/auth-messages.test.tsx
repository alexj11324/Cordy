// @vitest-environment jsdom
import { renderToString } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AuthMessagesProvider, useAuthMessages } from "./auth-messages";

function Message() { return <p>{useAuthMessages().preparing}</p>; }
describe("Accounts first-render language", () => {
  it("renders the chosen language on the server, before any browser effects", () => {
    expect(renderToString(<AuthMessagesProvider locale="zh-Hans"><Message /></AuthMessagesProvider>)).toContain("正在准备登录");
    expect(renderToString(<AuthMessagesProvider locale="en"><Message /></AuthMessagesProvider>)).toContain("Preparing sign-in");
  });
});
