"use client";

import { useEffect, useState } from "react";
import { apiClient } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card, CardHeader, CardContent } from "@/components/ui/card";
import type { MarketplaceVersion } from "@/types/api";

function formatBytes(n: number | null): string {
  if (n === null) return "—";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export default function AdminReviewPage() {
  const [versions, setVersions] = useState<MarketplaceVersion[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [actionId, setActionId] = useState<string | null>(null);

  useEffect(() => {
    apiClient
      .get<MarketplaceVersion[]>("/api/marketplace/review")
      .then(setVersions)
      .catch(() => setError("Failed to load review queue."))
      .finally(() => setLoading(false));
  }, []);

  async function handleApprove(id: string) {
    setActionId(id);
    try {
      await apiClient.post<unknown>(`/api/marketplace/review/${id}/approve`);
      setVersions((prev) => prev.filter((v) => v.id !== id));
    } catch {
      // leave as-is on error
    } finally {
      setActionId(null);
    }
  }

  async function handleReject(id: string) {
    const reason = window.prompt("Rejection reason (optional):");
    if (reason === null) return; // cancelled
    setActionId(id);
    try {
      await apiClient.post<unknown>(`/api/marketplace/review/${id}/reject`, {
        reason: reason.trim() || undefined,
      });
      setVersions((prev) => prev.filter((v) => v.id !== id));
    } catch {
      // leave as-is on error
    } finally {
      setActionId(null);
    }
  }

  return (
    <div className="mx-auto max-w-4xl px-4 py-12">
      <div className="mb-6">
        <h1 className="text-2xl font-bold">Review Queue</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Plugin versions pending approval.
        </p>
      </div>

      {loading && <p className="text-muted-foreground">Loading queue...</p>}
      {error && <p className="text-destructive">{error}</p>}

      {!loading && !error && versions.length === 0 && (
        <div className="rounded-lg border border-dashed py-16 text-center">
          <p className="text-muted-foreground">No versions pending review.</p>
        </div>
      )}

      <div className="flex flex-col gap-3">
        {versions.map((v) => (
          <Card key={v.id}>
            <CardHeader className="pb-2 pt-4">
              <div className="flex items-start justify-between gap-4">
                <div className="min-w-0 flex-1 text-sm">
                  <div className="flex items-center gap-2 font-medium">
                    <span className="font-mono">{v.plugin_id}</span>
                    <span className="text-muted-foreground">
                      v{v.version}
                    </span>
                    <span className="rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                      {v.platform}
                    </span>
                  </div>
                  <div className="mt-1 space-y-0.5 text-xs text-muted-foreground">
                    <p>Size: {formatBytes(v.size_bytes)}</p>
                    <p>
                      Submitted:{" "}
                      {new Date(v.created_at).toLocaleString()}
                    </p>
                  </div>
                </div>
                <CardContent className="flex shrink-0 items-center gap-2 p-0">
                  <Button
                    size="sm"
                    disabled={actionId === v.id}
                    onClick={() => handleApprove(v.id)}
                    className="bg-green-600 hover:bg-green-700"
                  >
                    Approve
                  </Button>
                  <Button
                    size="sm"
                    variant="destructive"
                    disabled={actionId === v.id}
                    onClick={() => handleReject(v.id)}
                  >
                    Reject
                  </Button>
                </CardContent>
              </div>
            </CardHeader>
          </Card>
        ))}
      </div>
    </div>
  );
}
