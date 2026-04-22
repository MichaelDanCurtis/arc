"use client";

import { useState, useEffect } from "react";
import { apiClient } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card, CardHeader, CardTitle, CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { PluginEntry, PluginConfigEntry } from "@/types/api";

export default function PluginsPage() {
  const [plugins, setPlugins] = useState<PluginEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [togglingId, setTogglingId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [configPlugin, setConfigPlugin] = useState<PluginEntry | null>(null);
  const [config, setConfig] = useState<PluginConfigEntry[]>([]);
  const [configLoading, setConfigLoading] = useState(false);

  useEffect(() => {
    apiClient
      .get<PluginEntry[]>("/api/plugins")
      .then(setPlugins)
      .catch(() => setError("Failed to load plugins."))
      .finally(() => setLoading(false));
  }, []);

  async function handleToggle(plugin: PluginEntry) {
    setTogglingId(plugin.id);
    try {
      const updated = await apiClient.patch<PluginEntry>(
        `/api/plugins/${plugin.id}`,
        { enabled: !plugin.enabled },
      );
      setPlugins((prev) => prev.map((p) => (p.id === updated.id ? updated : p)));
    } catch {
      // leave as-is on error
    } finally {
      setTogglingId(null);
    }
  }

  async function handleDelete(plugin: PluginEntry) {
    if (
      !window.confirm(
        `Delete plugin "${plugin.name}" (v${plugin.version})? This cannot be undone.`,
      )
    ) {
      return;
    }
    setDeletingId(plugin.id);
    try {
      await apiClient.del<unknown>(`/api/plugins/${plugin.id}`);
      setPlugins((prev) => prev.filter((p) => p.id !== plugin.id));
    } catch {
      // leave as-is on error
    } finally {
      setDeletingId(null);
    }
  }

  async function handleViewConfig(plugin: PluginEntry) {
    setConfigPlugin(plugin);
    setConfig([]);
    setConfigLoading(true);
    try {
      const entries = await apiClient.get<PluginConfigEntry[]>(
        `/api/plugins/${plugin.id}/config`,
      );
      setConfig(entries);
    } catch {
      setConfig([]);
    } finally {
      setConfigLoading(false);
    }
  }

  function formatDate(iso: string) {
    try {
      return new Date(iso).toLocaleString();
    } catch {
      return iso;
    }
  }

  return (
    <div className="mx-auto max-w-4xl">
      <div className="mb-6">
        <h1 className="text-2xl font-semibold">Plugins</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Manage installed plugins for your team.
        </p>
      </div>

      {loading && <p className="text-muted-foreground">Loading plugins...</p>}
      {error && <p className="text-destructive">{error}</p>}

      {!loading && !error && plugins.length === 0 && (
        <p className="py-8 text-center text-muted-foreground">
          No plugins installed yet.
        </p>
      )}

      <div className="flex flex-col gap-3">
        {plugins.map((plugin) => (
          <Card key={plugin.id}>
            <CardHeader className="pb-2 pt-4">
              <div className="flex items-start justify-between gap-4">
                <div className="min-w-0 flex-1">
                  <CardTitle className="flex items-center gap-2 text-base">
                    {plugin.name}
                    <span className="text-xs font-normal text-muted-foreground">
                      v{plugin.version}
                    </span>
                    <span
                      className={`rounded-full px-2 py-0.5 text-xs font-medium ${
                        plugin.enabled
                          ? "bg-green-500/20 text-green-400"
                          : "bg-muted text-muted-foreground"
                      }`}
                    >
                      {plugin.enabled ? "enabled" : "disabled"}
                    </span>
                  </CardTitle>
                  <div className="mt-1 space-y-0.5 text-xs text-muted-foreground">
                    {plugin.source_url && (
                      <p className="truncate">
                        Source:{" "}
                        <a
                          href={plugin.source_url}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="underline hover:text-foreground"
                        >
                          {plugin.source_url}
                        </a>
                      </p>
                    )}
                    <p>Installed: {formatDate(plugin.created_at)}</p>
                    {plugin.installed_by && (
                      <p>By: {plugin.installed_by}</p>
                    )}
                  </div>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => handleViewConfig(plugin)}
                  >
                    Config
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={togglingId === plugin.id}
                    onClick={() => handleToggle(plugin)}
                  >
                    {togglingId === plugin.id
                      ? "Saving..."
                      : plugin.enabled
                        ? "Disable"
                        : "Enable"}
                  </Button>
                  <Button
                    variant="destructive"
                    size="sm"
                    disabled={deletingId === plugin.id}
                    onClick={() => handleDelete(plugin)}
                  >
                    {deletingId === plugin.id ? "Deleting..." : "Delete"}
                  </Button>
                </div>
              </div>
            </CardHeader>
          </Card>
        ))}
      </div>

      {/* Config dialog */}
      <Dialog
        open={configPlugin !== null}
        onOpenChange={(open) => {
          if (!open) setConfigPlugin(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {configPlugin?.name} — Config
            </DialogTitle>
          </DialogHeader>

          {configLoading && (
            <p className="py-4 text-center text-sm text-muted-foreground">
              Loading config...
            </p>
          )}

          {!configLoading && config.length === 0 && (
            <p className="py-4 text-center text-sm text-muted-foreground">
              No config entries found.
            </p>
          )}

          {!configLoading && config.length > 0 && (
            <Card>
              <CardContent className="p-0">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b">
                      <th className="px-4 py-2 text-left font-medium text-muted-foreground">
                        Key
                      </th>
                      <th className="px-4 py-2 text-left font-medium text-muted-foreground">
                        Value
                      </th>
                      <th className="px-4 py-2 text-left font-medium text-muted-foreground">
                        Updated
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {config.map((entry) => (
                      <tr key={entry.config_key} className="border-b last:border-0">
                        <td className="px-4 py-2 font-mono text-xs">
                          {entry.config_key}
                        </td>
                        <td className="px-4 py-2 font-mono text-xs">
                          {JSON.stringify(entry.config_val)}
                        </td>
                        <td className="px-4 py-2 text-xs text-muted-foreground">
                          {formatDate(entry.updated_at)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </CardContent>
            </Card>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
