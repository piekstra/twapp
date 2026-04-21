import { describe, it, expect } from "vitest";
import { validateArgs, type SendArgs } from "./MessageComposer";

const base: SendArgs = {
  to: "reviewer",
  priority: "routine",
  body: "hello",
};

describe("validateArgs", () => {
  it("accepts minimal valid args", () => {
    expect(validateArgs(base)).toBeNull();
  });

  it("accepts broadcast (to: all)", () => {
    expect(validateArgs({ ...base, to: "all" })).toBeNull();
  });

  it("rejects empty recipient", () => {
    expect(validateArgs({ ...base, to: "" })).toMatch(/Recipient/i);
    expect(validateArgs({ ...base, to: "   " })).toMatch(/Recipient/i);
  });

  it("rejects empty body", () => {
    expect(validateArgs({ ...base, body: "" })).toMatch(/body/i);
    expect(validateArgs({ ...base, body: "\n  \n" })).toMatch(/body/i);
  });

  it("ignores optional fields when blank", () => {
    expect(
      validateArgs({ ...base, subject: "", thread: "", cc: "" }),
    ).toBeNull();
  });
});
