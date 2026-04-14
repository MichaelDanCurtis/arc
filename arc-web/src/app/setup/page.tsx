"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { getRelayUrl } from "@/lib/config";
import { setTokens } from "@/lib/auth";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import type {
  SetupRequest,
  SetupResponse,
  SetupStatus,
} from "@/types/api";

export default function SetupPage() {
  const router = useRouter();
  const [checking, setChecking] = useState(true);
  const [ownerName, setOwnerName] = useState("");
  const [ownerEmail, setOwnerEmail] = useState("");
  const [teamName, setTeamName] = useState("");
  const [password, setPassword] = useState("");
  const [passwordConfirm, setPasswordConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // On mount, verify setup is still needed. If already configured, bounce to login.
  useEffect(() => {
    const baseUrl = getRelayUrl();
    fetch(`${baseUrl}/api/setup/status`)
      .then((r) => (r.ok ? (r.json() as Promise<SetupStatus>) : null))
      .then((status) => {
        if (status?.is_configured) {
          router.replace("/login");
        } else {
          setChecking(false);
        }
      })
      .catch(() => setChecking(false));
  }, [router]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    if (password !== passwordConfirm) {
      setError("Passwords do not match");
      return;
    }
    if (password.length < 8) {
      setError("Password must be at least 8 characters");
      return;
    }
    setSubmitting(true);
    try {
      const body: SetupRequest = {
        owner_name: ownerName.trim(),
        owner_email: ownerEmail.trim(),
        team_name: teamName.trim(),
        password,
      };
      const res = await fetch(`${getRelayUrl()}/api/setup`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!res.ok) {
        const msg = await res.text();
        throw new Error(msg || `Setup failed (${res.status})`);
      }
      const data: SetupResponse = await res.json();
      setTokens(data.access_token, data.refresh_token);
      router.replace("/sessions");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Setup failed");
    } finally {
      setSubmitting(false);
    }
  }

  if (checking) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background">
        <p className="text-muted-foreground">Checking setup status…</p>
      </div>
    );
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-6">
      <Card className="w-full max-w-md">
        <CardHeader>
          <CardTitle className="text-2xl">Welcome to Arc</CardTitle>
          <p className="mt-1 text-sm text-muted-foreground">
            Set up the first owner account and team. This only runs once.
          </p>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="flex flex-col gap-4">
            <div className="flex flex-col gap-2">
              <Label htmlFor="owner-name">Your name</Label>
              <Input
                id="owner-name"
                type="text"
                value={ownerName}
                onChange={(e) => setOwnerName(e.target.value)}
                required
                autoFocus
              />
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="owner-email">Email</Label>
              <Input
                id="owner-email"
                type="email"
                value={ownerEmail}
                onChange={(e) => setOwnerEmail(e.target.value)}
                required
              />
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="team-name">Team name</Label>
              <Input
                id="team-name"
                type="text"
                value={teamName}
                onChange={(e) => setTeamName(e.target.value)}
                required
              />
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="password">Password</Label>
              <Input
                id="password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                required
                minLength={8}
              />
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="password-confirm">Confirm password</Label>
              <Input
                id="password-confirm"
                type="password"
                value={passwordConfirm}
                onChange={(e) => setPasswordConfirm(e.target.value)}
                required
                minLength={8}
              />
            </div>
            {error && <p className="text-sm text-destructive">{error}</p>}
            <Button type="submit" disabled={submitting}>
              {submitting ? "Creating account…" : "Complete setup"}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
