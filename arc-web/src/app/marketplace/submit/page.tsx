"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { apiClient } from "@/lib/api";
import type { CreatePluginRequest, MarketplacePluginFull } from "@/types/api";

function slugify(value: string): string {
  return value
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

const CATEGORIES = ["ai", "devops", "git", "monitoring", "utility"];

export default function SubmitPluginPage() {
  const router = useRouter();
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugManual, setSlugManual] = useState(false);
  const [description, setDescription] = useState("");
  const [category, setCategory] = useState("");
  const [homepageUrl, setHomepageUrl] = useState("");
  const [repositoryUrl, setRepositoryUrl] = useState("");
  const [license, setLicense] = useState("MIT");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function handleNameChange(value: string) {
    setName(value);
    if (!slugManual) {
      setSlug(slugify(value));
    }
  }

  function handleSlugChange(value: string) {
    setSlugManual(true);
    setSlug(value);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);

    const req: CreatePluginRequest = {
      name: name.trim(),
      slug: slug.trim(),
      description: description.trim() || undefined,
      category: category || undefined,
      homepage_url: homepageUrl.trim() || undefined,
      repository_url: repositoryUrl.trim() || undefined,
      license: license.trim() || undefined,
    };

    try {
      const plugin = await apiClient.post<MarketplacePluginFull>(
        "/api/marketplace/plugins",
        req,
      );
      router.push(`/submissions/${plugin.id}`);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create plugin.");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="mx-auto max-w-2xl px-4 py-12">
      <div className="mb-8">
        <h1 className="text-2xl font-bold">Submit a Plugin</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Publish your plugin to the Beacon marketplace.
        </p>
      </div>

      <form onSubmit={handleSubmit} className="flex flex-col gap-5">
        <div className="flex flex-col gap-1.5">
          <label className="text-sm font-medium" htmlFor="name">
            Plugin Name <span className="text-destructive">*</span>
          </label>
          <Input
            id="name"
            value={name}
            onChange={(e) => handleNameChange(e.target.value)}
            placeholder="My Awesome Plugin"
            required
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label className="text-sm font-medium" htmlFor="slug">
            Slug <span className="text-destructive">*</span>
          </label>
          <Input
            id="slug"
            value={slug}
            onChange={(e) => handleSlugChange(e.target.value)}
            placeholder="my-awesome-plugin"
            required
          />
          <p className="text-xs text-muted-foreground">
            Used in URLs. Auto-generated from name; edit to customise.
          </p>
        </div>

        <div className="flex flex-col gap-1.5">
          <label className="text-sm font-medium" htmlFor="description">
            Description
          </label>
          <textarea
            id="description"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="What does your plugin do?"
            rows={4}
            className="rounded-md border bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label className="text-sm font-medium" htmlFor="category">
            Category
          </label>
          <select
            id="category"
            value={category}
            onChange={(e) => setCategory(e.target.value)}
            className="rounded-md border bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
          >
            <option value="">— select —</option>
            {CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
        </div>

        <div className="flex flex-col gap-1.5">
          <label className="text-sm font-medium" htmlFor="homepage-url">
            Homepage URL
          </label>
          <Input
            id="homepage-url"
            type="url"
            value={homepageUrl}
            onChange={(e) => setHomepageUrl(e.target.value)}
            placeholder="https://example.com"
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label className="text-sm font-medium" htmlFor="repo-url">
            Repository URL
          </label>
          <Input
            id="repo-url"
            type="url"
            value={repositoryUrl}
            onChange={(e) => setRepositoryUrl(e.target.value)}
            placeholder="https://github.com/org/repo"
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label className="text-sm font-medium" htmlFor="license">
            License
          </label>
          <Input
            id="license"
            value={license}
            onChange={(e) => setLicense(e.target.value)}
            placeholder="MIT"
          />
        </div>

        {error && (
          <p className="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </p>
        )}

        <div className="flex gap-3 pt-2">
          <Button type="submit" disabled={submitting}>
            {submitting ? "Submitting..." : "Submit Plugin"}
          </Button>
          <Button
            type="button"
            variant="ghost"
            onClick={() => router.back()}
          >
            Cancel
          </Button>
        </div>
      </form>
    </div>
  );
}
