"use client";

import { type ReactNode } from "react";
import Link from "next/link";
import { Button } from "@/components/ui/button";
import { getAccessToken } from "@/lib/auth";
import { getRelayUrl } from "@/lib/config";

export default function MarketplaceLayout({ children }: { children: ReactNode }) {
  const isLoggedIn = typeof window !== "undefined" && !!getAccessToken();

  return (
    <div className="min-h-screen bg-background">
      <header className="sticky top-0 z-50 border-b bg-card">
        <div className="mx-auto flex h-14 max-w-6xl items-center justify-between px-4">
          <Link href="/" className="text-lg font-bold">
            <span className="text-primary">beam</span>
            <span className="text-muted-foreground">.ailuxis.com</span>
          </Link>
          <nav className="flex items-center gap-4 text-sm text-muted-foreground">
            <Link href="/" className="hover:text-foreground">Browse</Link>
            <Link href="/categories" className="hover:text-foreground">Categories</Link>
          </nav>
          <div className="flex gap-2">
            {isLoggedIn ? (
              <Button variant="outline" size="sm" asChild>
                <Link href="/submit">Submit Plugin</Link>
              </Button>
            ) : (
              <>
                <Button variant="ghost" size="sm" asChild>
                  <a href={`${getRelayUrl()}/api/auth/oauth/github`}>Sign In</a>
                </Button>
                <Button size="sm" asChild>
                  <a href={`${getRelayUrl()}/api/auth/oauth/github`}>Submit Plugin</a>
                </Button>
              </>
            )}
          </div>
        </div>
      </header>
      <main>{children}</main>
    </div>
  );
}
