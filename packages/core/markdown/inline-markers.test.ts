import { describe, expect, it } from "vitest";
import { insertMarkdownInlineMarkers } from "./inline-markers";

describe("insertMarkdownInlineMarkers", () => {
  it("inserts at an ordinary text boundary", () => {
    expect(
      insertMarkdownInlineMarkers("claim follows", [
        { offset: 5, markdown: " [1](https://source.example)" },
      ]),
    ).toBe("claim [1](https://source.example) follows");
  });

  it("moves a boundary inside inline code after the complete node", () => {
    expect(
      insertMarkdownInlineMarkers("use `claim` now", [
        { offset: 7, markdown: " [1](https://source.example)" },
      ]),
    ).toBe("use `claim` [1](https://source.example) now");
  });

  it("moves a boundary inside a link label after the complete link", () => {
    const content = "read [claim](https://target.example) now";
    expect(
      insertMarkdownInlineMarkers(content, [
        { offset: 9, markdown: " [1](https://source.example)" },
      ]),
    ).toBe(
      "read [claim](https://target.example) [1](https://source.example) now",
    );
  });

  it("keeps marker order when citations share a boundary", () => {
    expect(
      insertMarkdownInlineMarkers("claim", [
        { offset: 5, markdown: " [1](https://one.example)" },
        { offset: 5, markdown: " [2](https://two.example)" },
      ]),
    ).toBe("claim [1](https://one.example) [2](https://two.example)");
  });
});
