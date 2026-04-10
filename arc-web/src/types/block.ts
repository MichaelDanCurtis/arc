export interface HostInfo {
  hostname: string;
  os: string;
  arch: string;
  beacon_version: string;
}

export interface BlockHeader {
  id: string; // BlockId (ULID string)
  session_id: string;
  tab_id: string;
  command: string;
  cwd: string;
  started_at: number; // SystemTime → Unix timestamp in MessagePack
  env_snapshot: Record<string, string>;
  host_info: HostInfo;
}

export interface BlockFooter {
  id: string; // BlockId
  exit_code: number | null;
  ended_at: number;
  shadow_git_ref: string | null;
}

export interface Block {
  header: BlockHeader;
  output: Uint8Array; // raw bytes with ANSI
  footer: BlockFooter | null; // null while running
}
