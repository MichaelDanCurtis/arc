"use client";

import { useEffect, useState, useCallback } from "react";
import { apiClient } from "@/lib/api";
import { PluginCard } from "@/components/marketplace/plugin-card";
import { SearchBar } from "@/components/marketplace/search-bar";
import { CategoryChips } from "@/components/marketplace/category-chips";
import type { MarketplacePlugin, CategoriesResponse } from "@/types/api";

export default function MarketplaceBrowsePage() {
  const [plugins, setPlugins] = useState<MarketplacePlugin[]>([]);
  const [categories, setCategories] = useState<string[]>([]);
  const [search, setSearch] = useState("");
  const [category, setCategory] = useState<string | null>(null);
  const [sort, setSort] = useState("popular");
  const [loading, setLoading] = useState(true);

  const fetchPlugins = useCallback(async () => {
    setLoading(true);
    const params = new URLSearchParams();
    if (search) params.set("search", search);
    if (category) params.set("category", category);
    params.set("sort", sort);
    const qs = params.toString();
    try {
      const data = await apiClient.get<MarketplacePlugin[]>(
        `/api/marketplace/plugins${qs ? `?${qs}` : ""}`,
      );
      setPlugins(data);
    } catch {
      setPlugins([]);
    } finally {
      setLoading(false);
    }
  }, [search, category, sort]);

  useEffect(() => {
    apiClient
      .get<CategoriesResponse>("/api/marketplace/categories")
      .then((r) => setCategories(r.categories))
      .catch(() => {});
  }, []);

  useEffect(() => {
    fetchPlugins();
  }, [fetchPlugins]);

  return (
    <div className="mx-auto max-w-6xl px-4 py-12">
      <div className="mb-8 text-center">
        <h1 className="mb-2 text-3xl font-bold">Beacon Plugin Marketplace</h1>
        <p className="text-muted-foreground">
          Extend your terminal with AI, DevOps, monitoring, and more.
        </p>
      </div>

      <div className="mb-6 flex justify-center">
        <SearchBar onSearch={setSearch} />
      </div>

      <div className="mb-6 flex justify-center">
        <CategoryChips
          categories={categories}
          selected={category}
          onSelect={setCategory}
        />
      </div>

      <div className="mb-4 flex items-center justify-between">
        <span className="text-sm text-muted-foreground">
          {plugins.length} plugin{plugins.length !== 1 ? "s" : ""}
        </span>
        <select
          value={sort}
          onChange={(e) => setSort(e.target.value)}
          className="rounded border bg-card px-3 py-1 text-sm"
        >
          <option value="popular">Popular</option>
          <option value="recent">Recent</option>
          <option value="name">Name A-Z</option>
        </select>
      </div>

      {loading ? (
        <div className="py-20 text-center text-muted-foreground">Loading...</div>
      ) : plugins.length === 0 ? (
        <div className="py-20 text-center text-muted-foreground">No plugins found.</div>
      ) : (
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {plugins.map((p) => (
            <PluginCard key={p.id} plugin={p} />
          ))}
        </div>
      )}
    </div>
  );
}
