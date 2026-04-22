"use client";

import { useEffect, useState } from "react";
import { useParams } from "next/navigation";
import Link from "next/link";
import { apiClient } from "@/lib/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import type { MarketplacePlugin, MarketplaceVersion } from "@/types/api";

interface PluginDetail extends MarketplacePlugin {
  author_id: string;
  homepage_url: string | null;
  repository_url: string | null;
  status: string;
  created_at: string;
}

export default function PluginDetailPage() {
  const { slug } = useParams<{ slug: string }>();
  const [plugin, setPlugin] = useState<PluginDetail | null>(null);
  const [versions, setVersions] = useState<MarketplaceVersion[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!slug) return;
    apiClient
      .get<PluginDetail>(`/api/marketplace/plugins/${slug}`)
      .then(setPlugin)
      .catch(() => setError("Plugin not found"));
    apiClient
      .get<MarketplaceVersion[]>(`/api/marketplace/plugins/${slug}/versions`)
      .then(setVersions)
      .catch(() => {});
  }, [slug]);

  if (error) {
    return (
      <div className="mx-auto max-w-4xl px-4 py-20 text-center">
        <h2 className="mb-2 text-xl font-semibold">Plugin not found</h2>
        <Link href="/marketplace" className="text-primary hover:underline">
          &larr; Back to Browse
        </Link>
      </div>
    );
  }

  if (!plugin) {
    return (
      <div className="mx-auto max-w-4xl px-4 py-20 text-center text-muted-foreground">
        Loading...
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-4xl px-4 py-8">
      <Link
        href="/marketplace"
        className="mb-6 inline-block text-sm text-muted-foreground hover:text-foreground"
      >
        &larr; Back to Browse
      </Link>

      {/* Header: icon + name + author + install button */}
      <div className="mb-8 flex items-start gap-6">
        <div className="flex h-16 w-16 flex-shrink-0 items-center justify-center rounded-2xl bg-gradient-to-br from-primary/20 to-primary/5 text-3xl">
          {plugin.icon_url ? (
            <img src={plugin.icon_url} alt="" className="h-10 w-10 rounded-lg" />
          ) : (
            "\uD83D\uDD0C"
          )}
        </div>
        <div className="flex-1 min-w-0">
          <h1 className="text-2xl font-bold">{plugin.name}</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            by <strong>{plugin.author_name ?? "Unknown"}</strong>
            {plugin.category && (
              <>
                {" \xB7 "}
                <span className="capitalize">{plugin.category}</span>
              </>
            )}
            {" \xB7 "}
            {plugin.payment_type === "free"
              ? "Free"
              : `$${(plugin.price_cents / 100).toFixed(0)}/mo`}
            {plugin.latest_version && (
              <>
                {" \xB7 "}
                <span>v{plugin.latest_version}</span>
              </>
            )}
          </p>
        </div>
        <Button size="lg" className="flex-shrink-0 font-semibold">
          Install to Team &rarr;
        </Button>
      </div>

      {/* Main content + sidebar */}
      <div className="grid gap-6 md:grid-cols-[1fr_280px]">
        {/* Left: description + links + version history */}
        <div>
          <h3 className="mb-2 text-lg font-semibold">Description</h3>
          <p className="mb-6 text-muted-foreground">
            {plugin.description ?? "No description provided."}
          </p>

          {(plugin.homepage_url || plugin.repository_url) && (
            <div className="mb-6 flex gap-4">
              {plugin.homepage_url && (
                <a
                  href={plugin.homepage_url}
                  className="text-sm text-primary hover:underline"
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  Homepage &rarr;
                </a>
              )}
              {plugin.repository_url && (
                <a
                  href={plugin.repository_url}
                  className="text-sm text-primary hover:underline"
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  Repository &rarr;
                </a>
              )}
            </div>
          )}

          <Separator className="my-6" />

          <h3 className="mb-3 text-lg font-semibold">Version History</h3>
          {versions.length === 0 ? (
            <p className="text-sm text-muted-foreground">No versions available.</p>
          ) : (
            <div className="overflow-hidden rounded-lg border">
              <table className="w-full text-sm">
                <thead className="bg-muted/50">
                  <tr>
                    <th className="px-4 py-2 text-left font-medium text-muted-foreground">
                      Version
                    </th>
                    <th className="px-4 py-2 text-left font-medium text-muted-foreground">
                      Platform
                    </th>
                    <th className="px-4 py-2 text-left font-medium text-muted-foreground">
                      Downloads
                    </th>
                    <th className="px-4 py-2 text-left font-medium text-muted-foreground">
                      Released
                    </th>
                  </tr>
                </thead>
                <tbody className="divide-y">
                  {versions.map((v) => (
                    <tr key={v.id} className="hover:bg-muted/30 transition-colors">
                      <td className="px-4 py-3">
                        <span className="font-semibold">{v.version}</span>
                        {v.release_notes && (
                          <p className="mt-0.5 text-xs text-muted-foreground line-clamp-1">
                            {v.release_notes}
                          </p>
                        )}
                      </td>
                      <td className="px-4 py-3">
                        <Badge variant="outline" className="text-xs">
                          {v.platform}
                        </Badge>
                      </td>
                      <td className="px-4 py-3 text-muted-foreground">
                        {v.download_count.toLocaleString()}
                      </td>
                      <td className="px-4 py-3 text-muted-foreground">
                        {new Date(v.created_at).toLocaleDateString()}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>

        {/* Right: stats sidebar */}
        <Card className="h-fit p-5">
          <h4 className="mb-3 font-semibold">Stats</h4>
          <dl className="space-y-2 text-sm">
            <div className="flex justify-between border-b pb-2">
              <dt className="text-muted-foreground">Downloads</dt>
              <dd className="font-medium">{plugin.download_count.toLocaleString()}</dd>
            </div>
            <div className="flex justify-between border-b pb-2">
              <dt className="text-muted-foreground">Last Updated</dt>
              <dd>{new Date(plugin.updated_at).toLocaleDateString()}</dd>
            </div>
            {plugin.license && (
              <div className="flex justify-between border-b pb-2">
                <dt className="text-muted-foreground">License</dt>
                <dd>{plugin.license}</dd>
              </div>
            )}
            <div className="flex justify-between border-b pb-2">
              <dt className="text-muted-foreground">Category</dt>
              <dd className="capitalize">{plugin.category ?? "\u2014"}</dd>
            </div>
            <div className="flex justify-between border-b pb-2">
              <dt className="text-muted-foreground">Author</dt>
              <dd>{plugin.author_name ?? "Unknown"}</dd>
            </div>
            <div className="flex justify-between">
              <dt className="text-muted-foreground">Price</dt>
              <dd
                className={
                  plugin.payment_type === "free" ? "text-green-500 font-semibold" : ""
                }
              >
                {plugin.payment_type === "free"
                  ? "Free"
                  : `$${(plugin.price_cents / 100).toFixed(0)}/mo`}
              </dd>
            </div>
          </dl>
        </Card>
      </div>
    </div>
  );
}
