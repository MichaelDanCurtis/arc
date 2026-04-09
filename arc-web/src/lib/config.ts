export function getRelayUrl(): string {
  const url = process.env.NEXT_PUBLIC_ARC_RELAY_URL;
  if (!url) throw new Error("NEXT_PUBLIC_ARC_RELAY_URL is not set");
  return url.replace(/\/$/, "");
}

export function getWsUrl(): string {
  const relay = getRelayUrl();
  return relay.replace(/^http/, "ws") + "/ws";
}
