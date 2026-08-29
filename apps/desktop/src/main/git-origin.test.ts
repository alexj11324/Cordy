import { describe, expect, it } from "vitest";
import { parseGitConfigRemoteUrl, parseGitdirPointer } from "./git-origin";

describe("parseGitConfigRemoteUrl", () => {
  it("reads origin.url from a typical clone config", () => {
    const config = `[core]
	repositoryformatversion = 0
[remote "origin"]
	url = git@github.com:acme/api.git
	fetch = +refs/heads/*:refs/remotes/origin/*
[branch "main"]
	remote = origin
	merge = refs/heads/main
`;
    expect(parseGitConfigRemoteUrl(config)).toBe("git@github.com:acme/api.git");
  });

  it("reads an https origin and ignores other remotes", () => {
    const config = `[remote "upstream"]
	url = https://github.com/other/api.git
[remote "origin"]
	url = https://github.com/acme/api.git
`;
    expect(parseGitConfigRemoteUrl(config)).toBe(
      "https://github.com/acme/api.git",
    );
  });

  it("returns null when origin is missing", () => {
    expect(parseGitConfigRemoteUrl("[core]\n\trepositoryformatversion = 0\n")).toBeNull();
  });
});

describe("parseGitdirPointer", () => {
  it("reads a linked worktree gitdir file", () => {
    expect(parseGitdirPointer("gitdir: /Users/me/src/api/.git/worktrees/feature\n")).toBe(
      "/Users/me/src/api/.git/worktrees/feature",
    );
  });
});
