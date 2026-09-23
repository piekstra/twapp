import { describe, expect, it } from "vitest";
import { shortUrl } from "./Linkify";

describe("shortUrl", () => {
  it("keeps the host and the last path segment", () => {
    expect(shortUrl("https://support.example.com/support/ticket/case-53658")).toBe("support.example.com…/case-53658");
    expect(shortUrl("https://git.example.com/group/project/-/merge_requests/228")).toBe("git.example.com…/228");
    expect(shortUrl("https://www.example.com/docs")).toBe("example.com/docs");
    expect(shortUrl("https://example.com")).toBe("example.com");
    expect(shortUrl("not a url")).toBe("not a url");
  });
});
