import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AVATAR_SIZE_PX } from "@orvilo/ui/lib/avatar-size";
vi.mock("../../runtimes/components/provider-logo", () => ({ProviderLogo: ({provider,className}: {provider:string;className:string}) => <svg data-testid="provider-logo" data-provider={provider} className={className}/> }));
import { AgentProviderAvatar } from "./agent-provider-avatar";

describe("Agent provider identity shape", () => {
 it("keeps assigned provider artwork raw without a circular avatar shell", () => {
  const {container}=render(<AgentProviderAvatar provider="opencode" name="OpenCode" size="sm"/>);
  const identity=container.querySelector('[data-agent-identity="assigned"]');
  expect(identity).toHaveStyle({width:`${AVATAR_SIZE_PX.sm}px`,height:`${AVATAR_SIZE_PX.sm}px`});
  expect(identity).not.toHaveClass("rounded-full","bg-muted","overflow-hidden");
  expect(container.querySelector('[data-slot="avatar"]')).toBeNull();
  expect(screen.getByTestId("provider-logo")).toHaveClass("size-full");
 });
 it("only frames an explicitly unassigned identity", () => {
  const {container}=render(<AgentProviderAvatar name="Unassigned" unassigned size="sm"/>);
  expect(container.querySelector('[data-agent-identity="unassigned"]')).toHaveClass("rounded-full","bg-muted");
 });
});
