"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { apiClient } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card, CardHeader, CardTitle, CardContent } from "@/components/ui/card";
import type { MarketplacePluginFull } from "@/types/api";

function statusBadge(status: string) {
  const map: Record<string, string> = {
    draft: "bg-muted text-muted-foreground",
    pending_review: "bg-yellow-500/20 text-yellow-400",
    approved: "bg-green-500/20 text-green-400",
    rejected: "bg-red-500/20 text-red-400",
  };
  const cls = map[status] ?? "bg-muted text-muted-foreground";
  return (
    <span className={`rounded-full px-2 py-0.5 text-xs font-medium ${cls}`}>
      {status.replace("_", " ")}
    </span>
  );
}

export default function SubmissionsPage() {
  const [plugins, setPlugins] = useState<MarketplacePluginFull[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    apiClient
      .get<MarketplacePluginFull[]>("/api/marketplace/my-plugins")
      .then(setPlugins)
      .catch(() => setError("Failed to load submissions."))
      .finally(() => setLoading(false));
  }, []);

  return (
    <div className="mx-auto max-w-4xl px-4 py-12">
      <div className="mb-6 flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold">My Submissions</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Plugins you have submitted to the marketplace.
          </p>
        </div>
        <Button asChild>
          <Link href="/submit">+ New Plugin</Link>
        </Button>
      </div>

      {loading && (
        <p className="text-muted-foreground">Loading submissions...</p>
      )}
      {error && <p className="text-destructive">{error}</p>}

      {!loading && !error && plugins.length === 0 && (
        <div className="rounded-lg border border-dashed py-16 text-center">
          <p className="text-muted-foreground">
            You have not submitted any plugins yet.
          </p>
          <Button asChild className="mt-4">
            <Link href="/submit">Submit your first plugin</Link>
          </Button>
        </div>
      )}

      <div className="flex flex-col gap-3">
        {plugins.map((plugin) => (
          <Link
            key={plugin.id}
            href={`/submissions/${plugin.id}`}
            className="block"
          >
            <Card className="transition-colors hover:bg-muted/50">
              <CardHeader className="pb-2 pt-4">
                <div className="flex items-start justify-between gap-4">
                  <div className="min-w-0 flex-1">
                    <CardTitle className="flex items-center gap-2 text-base">
                      {plugin.name}
                      {statusBadge(plugin.status)}
                    </CardTitle>
                    <div className="mt-1 space-y-0.5 text-xs text-muted-foreground">
                      <p>
                        <span className="font-mono">{plugin.slug}</span>
                        {plugin.category && (
                          <span className="ml-2 rounded bg-muted px-1.5 py-0.5">
                            {plugin.category}
                          </span>
                        )}
                      </p>
                      {plugin.description && (
                        <p className="truncate">{plugin.description}</p>
                      )}
                    </div>
                  </div>
                  <CardContent className="p-0 text-xs text-muted-foreground">
                    {new Date(plugin.updated_at).toLocaleDateString()}
                  </CardContent>
                </div>
              </CardHeader>
            </Card>
          </Link>
        ))}
      </div>
    </div>
  );
}
