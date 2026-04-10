import AnsiToHtml from "ansi-to-html";

const converter = new AnsiToHtml({
  fg: "#d4d4d4",
  bg: "#1e1e1e",
  newline: true,
  escapeXML: true,
});

export function ansiToHtml(input: string | Uint8Array): string {
  const text = typeof input === "string" ? input : new TextDecoder().decode(input);
  if (text === "") return "";
  return converter.toHtml(text);
}
