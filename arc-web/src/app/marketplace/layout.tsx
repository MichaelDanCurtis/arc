"use client";

import { type ReactNode } from "react";
import Link from "next/link";
import { buttonVariants } from "@/components/ui/button";
import { getAccessToken } from "@/lib/auth";
import { getRelayUrl } from "@/lib/config";
import { cn } from "@/lib/utils";

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
            {isLoggedIn && (
              <Link href="/submissions" className="hover:text-foreground">My Submissions</Link>
            )}
          </nav>
          <div className="flex gap-2">
            {isLoggedIn ? (
              <Link href="/submit" className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
                Submit Plugin
              </Link>
            ) : (
              <>
                <a
                  href={`${getRelayUrl()}/api/auth/oauth/github`}
                  className={cn(buttonVariants({ variant: "ghost", size: "sm" }))}
                >
                  Sign In
                </a>
                <a
                  href={`${getRelayUrl()}/api/auth/oauth/github`}
                  className={cn(buttonVariants({ size: "sm" }))}
                >
                  Submit Plugin
                </a>
              </>
            )}
          </div>
        </div>
      </header>
      <main>{children}</main>
    </div>
  );
}
