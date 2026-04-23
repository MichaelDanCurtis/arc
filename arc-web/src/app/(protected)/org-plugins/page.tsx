"use client";

import { useState } from "react";
import { apiClient } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardHeader, CardContent } from "@/components/ui/card";

interface AllowedPlugin {
  plugin_id: string;
  added_at: string;
}

export default function OrgPluginsPage() {
  const [allowed, setAllowed] = useState<AllowedPlugin[]>([]);
  const [pluginId, setPluginId] = useState("");
  const [adding, setAdding] = useState(false);
  const [removingId, setRemovingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Org detection from hostname is not yet wired — show placeholder
  const orgSlug: string | null = null;

  async function handleAdd(e: React.FormEvent) {
    e.preventDefault();
    if (!pluginId.trim()) return;
    setError(null);
    setAdding(true);
    try {
      const entry = await apiClient.post<AllowedPlugin>(
        "/api/org/plugins/allowlist",
        { plugin_id: pluginId.trim() },
      );
      setAllowed((prev) => [...prev, entry]);
      setPluginId("");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to add plugin.");
    } finally {
      setAdding(false);
    }
  }

  async function handleRemove(id: string) {
    setRemovingId(id);
    try {
      await apiClient.del<unknown>(`/api/org/plugins/allowlist/${id}`);
      setAllowed((prev) => prev.filter((p) => p.plugin_id !== id));
    } catch {
      // leave as-is
    } finally {
      setRemovingId(null);
    }
  }

  return (
    <div className="mx-auto max-w-3xl">
      <div className="mb-6">
        <h1 className="text-2xl font-semibold">Org Plugins</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Manage the plugin allowlist for your organisation.
        </p>
      </div>

      {orgSlug === null && (
        <div className="mb-6 rounded-lg border border-dashed px-4 py-6 text-center text-sm text-muted-foreground">
          Select an org to manage its plugin allowlist. Org detection from
          hostname is not yet wired — this will resolve automatically once your
          Arc instance is connected.
        </div>
      )}

      <Card className="mb-6">
        <CardHeader className="pb-2 pt-4 text-sm font-medium">
          Add Plugin to Allowlist
        </CardHeader>
        <CardContent>
          <form onSubmit={handleAdd} className="flex gap-3">
            <Input
              value={pluginId}
              onChange={(e) => setPluginId(e.target.value)}
              placeholder="Plugin ID (UUID)"
              className="flex-1"
              required
            />
            <Button type="submit" disabled={adding}>
              {adding ? "Adding..." : "Add"}
            </Button>
          </form>
          {error && (
            <p className="mt-2 text-sm text-destructive">{error}</p>
          )}
        </CardContent>
      </Card>

      <div>
        <h2 className="mb-3 text-base font-semibold">Allowed Plugins</h2>
        {allowed.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No plugins in the allowlist yet.
          </p>
        ) : (
          <div className="flex flex-col gap-2">
            {allowed.map((p) => (
              <Card key={p.plugin_id}>
                <CardHeader className="py-3">
                  <div className="flex items-center justify-between gap-4">
                    <div className="min-w-0 flex-1">
                      <p className="truncate font-mono text-sm">{p.plugin_id}</p>
                      <p className="text-xs text-muted-foreground">
                        Added {new Date(p.added_at).toLocaleString()}
                      </p>
                    </div>
                    <Button
                      size="sm"
                      variant="destructive"
                      disabled={removingId === p.plugin_id}
                      onClick={() => handleRemove(p.plugin_id)}
                    >
                      {removingId === p.plugin_id ? "Removing..." : "Remove"}
                    </Button>
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
