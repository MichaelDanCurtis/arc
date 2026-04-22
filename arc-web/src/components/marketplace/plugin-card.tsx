import Link from "next/link";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import type { MarketplacePlugin } from "@/types/api";

const categoryColors: Record<string, string> = {
  ai: "bg-blue-500/10 text-blue-400",
  devops: "bg-orange-500/10 text-orange-400",
  git: "bg-purple-500/10 text-purple-400",
  monitoring: "bg-green-500/10 text-green-400",
  utility: "bg-red-500/10 text-red-400",
};

function formatDownloads(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

export function PluginCard({ plugin }: { plugin: MarketplacePlugin }) {
  const colorClass =
    categoryColors[plugin.category || ""] || "bg-muted text-muted-foreground";

  return (
    <Link href={`/plugins/${plugin.slug}`}>
      <Card className="group relative cursor-pointer p-5 transition-all hover:-translate-y-0.5 hover:border-primary hover:shadow-lg">
        {plugin.category && (
          <Badge
            variant="outline"
            className={`absolute right-3 top-3 text-[10px] uppercase ${colorClass}`}
          >
            {plugin.category}
          </Badge>
        )}
        <div className="mb-3 flex items-center gap-3">
          <div className="flex h-11 w-11 items-center justify-center rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 text-xl">
            {plugin.icon_url ? (
              // eslint-disable-next-line @next/next/no-img-element
              <img src={plugin.icon_url} alt="" className="h-7 w-7 rounded" />
            ) : (
              "\uD83D\uDD0C"
            )}
          </div>
          <div>
            <div className="font-semibold text-sm">{plugin.name}</div>
            <div className="text-xs text-muted-foreground">
              by {plugin.author_name || "Unknown"}
            </div>
          </div>
        </div>
        <p className="mb-3 line-clamp-2 text-xs text-muted-foreground">
          {plugin.description || "No description"}
        </p>
        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span>&darr; {formatDownloads(plugin.download_count)}</span>
          <span
            className={`font-semibold ${
              plugin.payment_type === "free" ? "text-green-400" : "text-orange-400"
            }`}
          >
            {plugin.payment_type === "free"
              ? "FREE"
              : `$${(plugin.price_cents / 100).toFixed(0)}/mo`}
          </span>
        </div>
      </Card>
    </Link>
  );
}
