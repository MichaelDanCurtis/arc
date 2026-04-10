import { Badge } from "@/components/ui/badge";
import type { BlockHeader as BlockHeaderType, BlockFooter } from "@/types/block";
import { cn } from "@/lib/utils";

interface Props {
  header: BlockHeaderType;
  footer: BlockFooter | null;
}

export function BlockHeaderBar({ header, footer }: Props) {
  const time = new Date(header.started_at).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });

  const exitCode = footer?.exit_code;
  const isError = exitCode !== null && exitCode !== undefined && exitCode !== 0;

  return (
    <div className="flex items-center gap-2 px-3 py-1.5 text-xs text-muted-foreground">
      <span>{time}</span>
      {exitCode !== null && exitCode !== undefined && (
        <Badge
          variant="outline"
          className={cn(
            "font-mono text-xs",
            isError ? "border-destructive text-destructive" : "border-green-600 text-green-600",
          )}
        >
          {exitCode}
        </Badge>
      )}
      <span className="font-mono">{header.cwd}</span>
      {footer?.shadow_git_ref && (
        <span className="font-mono text-blue-400">{footer.shadow_git_ref}</span>
      )}
    </div>
  );
}
