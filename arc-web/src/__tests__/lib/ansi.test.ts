import { describe, it, expect } from "vitest";
import { ansiToHtml } from "@/lib/ansi";

describe("ansiToHtml", () => {
  it("converts plain text without changes", () => {
    const html = ansiToHtml("hello world");
    expect(html).toContain("hello world");
  });

  it("converts ANSI bold", () => {
    const html = ansiToHtml("\x1b[1mbold text\x1b[0m");
    expect(html).toContain("bold text");
    expect(html).toContain("<"); // Should have HTML tags
  });

  it("converts ANSI green", () => {
    const html = ansiToHtml("\x1b[32mgreen text\x1b[0m");
    expect(html).toContain("green text");
  });

  it("handles empty input", () => {
    const html = ansiToHtml("");
    expect(html).toBe("");
  });

  it("converts bytes (Uint8Array) to HTML", () => {
    const bytes = new TextEncoder().encode("hello \x1b[31mred\x1b[0m");
    const html = ansiToHtml(bytes);
    expect(html).toContain("hello");
    expect(html).toContain("red");
  });
});
