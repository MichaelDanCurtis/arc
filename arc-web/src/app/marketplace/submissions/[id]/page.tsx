"use client";

import { useEffect, useRef, useState } from "react";
import { useParams, useRouter } from "next/navigation";
import { apiClient } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardHeader, CardTitle, CardContent } from "@/components/ui/card";
import type { MarketplacePluginFull, MarketplaceVersion } from "@/types/api";

const PLATFORMS = [
  "darwin-universal",
  "darwin-arm64",
  "darwin-x86_64",
  "linux-x86_64",
  "linux-arm64",
];

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

function formatBytes(n: number | null): string {
  if (n === null) return "—";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export default function SubmissionDetailPage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();

  const [plugin, setPlugin] = useState<MarketplacePluginFull | null>(null);
  const [versions, setVersions] = useState<MarketplaceVersion[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Upload form state
  const [version, setVersion] = useState("");
  const [platform, setPlatform] = useState(PLATFORMS[0]);
  const fileRef = useRef<HTMLInputElement>(null);
  const [uploading, setUploading] = useState(false);
  const [uploadError, setUploadError] = useState<string | null>(null);

  // Deleting state
  const [deleting, setDeleting] = useState(false);

  useEffect(() => {
    async function load() {
      try {
        const all = await apiClient.get<MarketplacePluginFull[]>(
          "/api/marketplace/my-plugins",
        );
        const found = all.find((p) => p.id === id) ?? null;
        setPlugin(found);
        if (found) {
          const vers = await apiClient.get<MarketplaceVersion[]>(
            `/api/marketplace/plugins/${id}/versions`,
          );
          setVersions(vers);
        }
      } catch {
        setError("Failed to load plugin.");
      } finally {
        setLoading(false);
      }
    }
    load();
  }, [id]);

  async function handleDelete() {
    if (!window.confirm(`Delete plugin "${plugin?.name}"? This cannot be undone.`)) return;
    setDeleting(true);
    try {
      await apiClient.del<unknown>(`/api/marketplace/plugins/${id}`);
      router.push("/submissions");
    } catch {
      setDeleting(false);
    }
  }

  async function handleUpload(e: React.FormEvent) {
    e.preventDefault();
    setUploadError(null);
    const file = fileRef.current?.files?.[0];
    if (!file) {
      setUploadError("Please select a .beaconplug file.");
      return;
    }
    setUploading(true);
    const formData = new FormData();
    formData.append("version", version.trim());
    formData.append("platform", platform);
    formData.append("file", file);
    try {
      const newVersion = await apiClient.upload<MarketplaceVersion>(
        `/api/marketplace/plugins/${id}/versions`,
        formData,
      );
      setVersions((prev) => [newVersion, ...prev]);
      setVersion("");
      setPlatform(PLATFORMS[0]);
      if (fileRef.current) fileRef.current.value = "";
    } catch (err) {
      setUploadError(err instanceof Error ? err.message : "Upload failed.");
    } finally {
      setUploading(false);
    }
  }

  if (loading) {
    return (
      <div className="mx-auto max-w-3xl px-4 py-12 text-muted-foreground">
        Loading...
      </div>
    );
  }

  if (error || !plugin) {
    return (
      <div className="mx-auto max-w-3xl px-4 py-12">
        <p className="text-destructive">{error ?? "Plugin not found."}</p>
        <Button variant="ghost" className="mt-4" onClick={() => router.push("/submissions")}>
          Back to submissions
        </Button>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-3xl px-4 py-12">
      {/* Header */}
      <div className="mb-8 flex items-start justify-between gap-4">
        <div>
          <div className="mb-1 flex items-center gap-2">
            <h1 className="text-2xl font-bold">{plugin.name}</h1>
            {statusBadge(plugin.status)}
          </div>
          <p className="font-mono text-sm text-muted-foreground">{plugin.slug}</p>
          {plugin.description && (
            <p className="mt-2 text-sm text-muted-foreground">{plugin.description}</p>
          )}
        </div>
        <Button
          variant="destructive"
          size="sm"
          disabled={deleting}
          onClick={handleDelete}
        >
          {deleting ? "Deleting..." : "Delete"}
        </Button>
      </div>

      {/* Upload form */}
      <Card className="mb-8">
        <CardHeader>
          <CardTitle className="text-base">Upload New Version</CardTitle>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleUpload} className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <label className="text-sm font-medium" htmlFor="version">
                Version <span className="text-destructive">*</span>
              </label>
              <Input
                id="version"
                value={version}
                onChange={(e) => setVersion(e.target.value)}
                placeholder="1.0.0"
                required
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <label className="text-sm font-medium" htmlFor="platform">
                Platform <span className="text-destructive">*</span>
              </label>
              <select
                id="platform"
                value={platform}
                onChange={(e) => setPlatform(e.target.value)}
                className="rounded-md border bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
              >
                {PLATFORMS.map((p) => (
                  <option key={p} value={p}>
                    {p}
                  </option>
                ))}
              </select>
            </div>

            <div className="flex flex-col gap-1.5">
              <label className="text-sm font-medium" htmlFor="file">
                Plugin File (.beaconplug) <span className="text-destructive">*</span>
              </label>
              <input
                id="file"
                type="file"
                accept=".beaconplug"
                ref={fileRef}
                className="text-sm text-muted-foreground file:mr-3 file:rounded file:border file:bg-muted file:px-3 file:py-1 file:text-sm file:font-medium"
              />
            </div>

            {uploadError && (
              <p className="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
                {uploadError}
              </p>
            )}

            <div>
              <Button type="submit" disabled={uploading}>
                {uploading ? "Uploading..." : "Upload Version"}
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>

      {/* Version list */}
      <div>
        <h2 className="mb-3 text-lg font-semibold">Versions</h2>
        {versions.length === 0 ? (
          <p className="text-sm text-muted-foreground">No versions uploaded yet.</p>
        ) : (
          <div className="flex flex-col gap-2">
            {versions.map((v) => (
              <Card key={v.id}>
                <CardHeader className="py-3">
                  <div className="flex items-center justify-between gap-4">
                    <div className="flex items-center gap-2 text-sm">
                      <span className="font-mono font-medium">v{v.version}</span>
                      <span className="text-muted-foreground">{v.platform}</span>
                      <span className="text-muted-foreground">{formatBytes(v.size_bytes)}</span>
                    </div>
                    <div className="flex items-center gap-2">
                      {statusBadge(v.status)}
                      <span className="text-xs text-muted-foreground">
                        {new Date(v.created_at).toLocaleDateString()}
                      </span>
                    </div>
                  </div>
                </CardHeader>
              </Card>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
