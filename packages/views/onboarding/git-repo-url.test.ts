import { describe, expect, it } from "vitest";
import { parseGitRepoUrl } from "./git-repo-url";

describe("parseGitRepoUrl", () => {
  it("reads https GitHub URLs", () => {
    expect(parseGitRepoUrl("https://github.com/acme/api.git")).toEqual({
      url: "https://github.com/acme/api.git",
      name: "api",
    });
  });

  it("reads scp-style SSH URLs", () => {
    expect(parseGitRepoUrl("git@github.com:acme/web.git")).toEqual({
      url: "git@github.com:acme/web.git",
      name: "web",
    });
  });

  it("rejects empty or spaced input", () => {
    expect(parseGitRepoUrl("")).toBeNull();
    expect(parseGitRepoUrl("not a url")).toBeNull();
  });
});
