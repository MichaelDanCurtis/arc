import Link from "next/link";
import { Badge } from "@/components/ui/badge";
import type { SessionResponse } from "@/types/api";
import { cn } from "@/lib/utils";

const stateColors: Record<string, string> = {
  active: "bg-green-500",
  suspended: "bg-yellow-500",
  ended: "bg-gray-400",
};

function timeAgo(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime();
  const seconds = Math.floor(diff / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

export function SessionCard({ session }: { session: SessionResponse }) {
  return (
    <Link
      href={`/sessions/${session.id}`}
      className="flex items-center gap-3 rounded-md border p-4 transition-colors hover:bg-accent"
    >
      <span
        className={cn(
          "h-2.5 w-2.5 rounded-full",
          stateColors[session.state] ?? "bg-gray-400",
        )}
      />
      <span className="flex-1 font-medium">
        {session.name ?? session.id}
      </span>
      <Badge variant="outline">{session.state}</Badge>
      <span className="text-sm text-muted-foreground">
        {timeAgo(session.last_seen_at)}
      </span>
    </Link>
  );
}
