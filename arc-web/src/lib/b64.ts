/**
 * Base64 encoding/decoding helpers for arc JSON-RPC binary payloads.
 *
 * arc.* notifications carry binary data (PTY bytes, input bytes) as
 * base64-encoded strings in their params. These helpers convert between
 * the wire representation and the Uint8Array expected by downstream consumers
 * (xterm, useMirrorBlocks, etc.).
 */

export function b64ToBytes(s: string): Uint8Array {
  const binary = atob(s);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export function bytesToB64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
  return btoa(s);
}
