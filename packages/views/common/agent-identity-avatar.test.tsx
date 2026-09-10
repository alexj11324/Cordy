import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({runtimeId: "codex-runtime"}));
vi.mock("@orvilo/core/hooks", () => ({useWorkspaceId: () => "workspace"}));
vi.mock("@orvilo/core/workspace/queries", () => ({agentListOptions: () => ({queryKey: ["agents"]})}));
vi.mock("@orvilo/core/runtimes", () => ({runtimeListOptions: () => ({queryKey: ["runtimes"]})}));
vi.mock("@tanstack/react-query", () => ({useQuery: ({queryKey}: {queryKey:string[]}) => ({data: queryKey[0] === "agents" ? [{id:"agent",name:"My agent",runtime_id:state.runtimeId,avatar_url:null}] : [{id:"codex-runtime",provider:"codex"},{id:"claude-runtime",provider:"claude"}]})}));
vi.mock("../agents/components/agent-provider-avatar", () => ({AgentProviderAvatar: ({provider,name}: {provider:string;name:string}) => <span data-testid="provider-identity" data-provider={provider}>{name}</span>}));
import { AgentIdentityAvatar } from "./agent-identity-avatar";

describe("shared agent identity", () => {
 it("uses the Agent card Harness identity when no avatar URL was saved", () => {
  state.runtimeId="codex-runtime";
  render(<AgentIdentityAvatar agentId="agent"/>);
  expect(screen.getByTestId("provider-identity")).toHaveAttribute("data-provider","codex");
 });
 it("updates every identity consumer when its runtime binding changes", () => {
  state.runtimeId="codex-runtime";
  const {rerender}=render(<AgentIdentityAvatar agentId="agent"/>);
  state.runtimeId="claude-runtime";
  rerender(<AgentIdentityAvatar agentId="agent"/>);
  expect(screen.getByTestId("provider-identity")).toHaveAttribute("data-provider","claude");
 });
});
