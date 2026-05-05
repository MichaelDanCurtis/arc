/**
 * Tests for arc-web/src/lib/b64.ts
 *
 * Verifies round-trip correctness and a known-vector pair.
 */
import { describe, it, expect } from "vitest";
import { b64ToBytes, bytesToB64 } from "@/lib/b64";

describe("b64ToBytes", () => {
  it("decodes a known base64 string to the correct bytes", () => {
    // "Hi" in base64 is "SGk="
    const result = b64ToBytes("SGk=");
    expect(result).toBeInstanceOf(Uint8Array);
    expect(result).toEqual(new Uint8Array([72, 105]));
  });

  it("decodes empty string to empty Uint8Array", () => {
    const result = b64ToBytes("");
    expect(result).toBeInstanceOf(Uint8Array);
    expect(result.length).toBe(0);
  });
});

describe("bytesToB64", () => {
  it("encodes known bytes to the correct base64 string", () => {
    // [72, 105] == "Hi" → "SGk="
    const result = bytesToB64(new Uint8Array([72, 105]));
    expect(result).toBe("SGk=");
  });

  it("encodes empty Uint8Array to empty string", () => {
    const result = bytesToB64(new Uint8Array(0));
    expect(result).toBe("");
  });
});

describe("round-trip", () => {
  it("b64ToBytes(bytesToB64(bytes)) === original bytes", () => {
    const original = new Uint8Array([0x00, 0x01, 0x7f, 0x80, 0xff, 0x48, 0x65, 0x6c, 0x6c, 0x6f]);
    const roundTripped = b64ToBytes(bytesToB64(original));
    expect(roundTripped).toEqual(original);
  });

  it("bytesToB64(b64ToBytes(str)) === original string", () => {
    const original = "SGVsbG8gV29ybGQ="; // "Hello World"
    const roundTripped = bytesToB64(b64ToBytes(original));
    expect(roundTripped).toBe(original);
  });
});
