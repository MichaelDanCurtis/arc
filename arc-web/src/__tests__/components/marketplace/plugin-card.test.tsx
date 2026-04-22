import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { PluginCard } from "@/components/marketplace/plugin-card";
import type { MarketplacePlugin } from "@/types/api";

vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) => (
    <a href={href}>{children}</a>
  ),
}));

const basePlugin: MarketplacePlugin = {
  id: "plugin-1",
  name: "Test Plugin",
  slug: "test-plugin",
  description: "A test plugin description",
  category: "ai",
  icon_url: null,
  homepage_url: null,
  repository_url: null,
  license: "MIT",
  payment_type: "free",
  price_cents: 0,
  author_name: "Jane Doe",
  download_count: 12300,
  latest_version: "1.0.0",
  updated_at: "2026-01-01T00:00:00Z",
};

describe("PluginCard", () => {
  it("renders plugin name and author", () => {
    render(<PluginCard plugin={basePlugin} />);
    expect(screen.getByText("Test Plugin")).toBeInTheDocument();
    expect(screen.getByText("by Jane Doe")).toBeInTheDocument();
  });

  it("renders description", () => {
    render(<PluginCard plugin={basePlugin} />);
    expect(screen.getByText("A test plugin description")).toBeInTheDocument();
  });

  it("renders formatted download count", () => {
    render(<PluginCard plugin={basePlugin} />);
    expect(screen.getByText("↓ 12.3k")).toBeInTheDocument();
  });

  it("renders FREE for free plugins", () => {
    render(<PluginCard plugin={basePlugin} />);
    expect(screen.getByText("FREE")).toBeInTheDocument();
  });

  it("renders price for paid plugins", () => {
    const paidPlugin: MarketplacePlugin = {
      ...basePlugin,
      payment_type: "paid",
      price_cents: 500,
    };
    render(<PluginCard plugin={paidPlugin} />);
    expect(screen.getByText("$5/mo")).toBeInTheDocument();
  });

  it("renders category badge", () => {
    render(<PluginCard plugin={basePlugin} />);
    expect(screen.getByText("ai")).toBeInTheDocument();
  });
});
