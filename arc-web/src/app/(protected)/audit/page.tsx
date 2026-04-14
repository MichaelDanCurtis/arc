"use client";

import { useCallback, useEffect, useState } from "react";
import { apiClient } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card } from "@/components/ui/card";
import type { AuditEntry } from "@/types/api";

const PAGE_SIZE = 50;

export default function AuditPage() {
  const [entries, setEntries] = useState<AuditEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [actorFilter, setActorFilter] = useState("");
  const [actionFilter, setActionFilter] = useState("");
  const [offset, setOffset] = useState(0);
  const [hasMore, setHasMore] = useState(false);

  const load = useCallback(
    async (reset: boolean) => {
      setLoading(true);
      setError(null);
      const nextOffset = reset ? 0 : offset;
      const params = new URLSearchParams({
        limit: String(PAGE_SIZE),
        offset: String(nextOffset),
      });
      if (actorFilter.trim()) params.set("actor", actorFilter.trim());
      if (actionFilter.trim()) params.set("action", actionFilter.trim());
      try {
        const rows = await apiClient.get<AuditEntry[]>(
          `/api/audit?${params.toString()}`,
        );
        setEntries((prev) => (reset ? rows : [...prev, ...rows]));
        setHasMore(rows.length === PAGE_SIZE);
        setOffset(nextOffset + rows.length);
      } catch (e) {
        setError(e instanceof Error ? e.message : "Failed to load audit log");
      } finally {
        setLoading(false);
      }
    },
    [actorFilter, actionFilter, offset],
  );

  useEffect(() => {
    load(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function handleApplyFilters(e: React.FormEvent) {
    e.preventDefault();
    setOffset(0);
    load(true);
  }

  function handleClearFilters() {
    setActorFilter("");
    setActionFilter("");
    setOffset(0);
    // Defer one tick so state changes take effect before load reads them.
    setTimeout(() => load(true), 0);
  }

  return (
    <div className="mx-auto max-w-5xl">
      <div className="mb-6">
        <h1 className="text-2xl font-semibold">Audit log</h1>
        <p className="text-sm text-muted-foreground">
          A record of authentication events, team changes, and other security-relevant actions.
        </p>
      </div>

      <form
        onSubmit={handleApplyFilters}
        className="mb-6 flex flex-wrap items-end gap-3"
      >
        <div className="flex flex-col gap-1">
          <label className="text-xs text-muted-foreground">Actor</label>
          <Input
            placeholder="user id or label"
            value={actorFilter}
            onChange={(e) => setActorFilter(e.target.value)}
            className="w-64"
          />
        </div>
        <div className="flex flex-col gap-1">
          <label className="text-xs text-muted-foreground">Action</label>
          <Input
            placeholder="user.login, team.create, …"
            value={actionFilter}
            onChange={(e) => setActionFilter(e.target.value)}
            className="w-56"
          />
        </div>
        <Button type="submit" disabled={loading}>
          Apply
        </Button>
        <Button
          type="button"
          variant="ghost"
          disabled={loading}
          onClick={handleClearFilters}
        >
          Clear
        </Button>
      </form>

      {error && (
        <Card className="mb-4 border-destructive bg-destructive/10 p-4 text-sm text-destructive">
          {error}
        </Card>
      )}

      <Card className="overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead className="border-b bg-muted/40">
              <tr className="text-left">
                <th className="px-4 py-2 font-medium">When</th>
                <th className="px-4 py-2 font-medium">Actor</th>
                <th className="px-4 py-2 font-medium">Action</th>
                <th className="px-4 py-2 font-medium">Resource</th>
                <th className="px-4 py-2 font-medium">Detail</th>
              </tr>
            </thead>
            <tbody>
              {entries.map((e) => (
                <tr key={e.id} className="border-b last:border-none">
                  <td className="whitespace-nowrap px-4 py-2 text-muted-foreground">
                    {new Date(e.timestamp_ms).toLocaleString()}
                  </td>
                  <td className="px-4 py-2 font-mono text-xs">
                    {truncate(e.actor, 36)}
                  </td>
                  <td className="px-4 py-2">
                    <code className="rounded bg-muted px-1.5 py-0.5 text-xs">
                      {e.action}
                    </code>
                  </td>
                  <td className="px-4 py-2 font-mono text-xs">
                    {e.resource || "—"}
                  </td>
                  <td className="px-4 py-2 text-muted-foreground">
                    {e.detail || "—"}
                  </td>
                </tr>
              ))}
              {!loading && entries.length === 0 && (
                <tr>
                  <td
                    colSpan={5}
                    className="px-4 py-8 text-center text-muted-foreground"
                  >
                    No audit entries match the current filters.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </Card>

      <div className="mt-4 flex items-center justify-between">
        <p className="text-xs text-muted-foreground">
          {entries.length} {entries.length === 1 ? "entry" : "entries"} shown
        </p>
        {hasMore && (
          <Button
            variant="outline"
            size="sm"
            onClick={() => load(false)}
            disabled={loading}
          >
            {loading ? "Loading..." : "Load more"}
          </Button>
        )}
      </div>
    </div>
  );
}

function truncate(s: string, n: number): string {
  return s.length <= n ? s : `${s.slice(0, n - 1)}…`;
}
