"use client";

import { useState, useEffect } from "react";
import Link from "next/link";
import { apiClient } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import type { TeamResponse } from "@/types/api";

export default function TeamsPage() {
  const [teams, setTeams] = useState<TeamResponse[]>([]);
  const [loading, setLoading] = useState(true);
  const [newTeamName, setNewTeamName] = useState("");
  const [creating, setCreating] = useState(false);
  const [dialogOpen, setDialogOpen] = useState(false);

  useEffect(() => {
    apiClient.get<TeamResponse[]>("/api/teams").then(setTeams).finally(() => setLoading(false));
  }, []);

  async function handleCreate() {
    if (!newTeamName.trim()) return;
    setCreating(true);
    try {
      const team = await apiClient.post<TeamResponse>("/api/teams", {
        name: newTeamName.trim(),
      });
      setTeams((prev) => [...prev, team]);
      setNewTeamName("");
      setDialogOpen(false);
    } finally {
      setCreating(false);
    }
  }

  return (
    <div className="mx-auto max-w-3xl">
      <div className="mb-6 flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Teams</h1>
        <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
          <DialogTrigger render={<Button />}>
            Create Team
          </DialogTrigger>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Create a new team</DialogTitle>
            </DialogHeader>
            <div className="flex flex-col gap-4">
              <Input
                placeholder="Team name"
                value={newTeamName}
                onChange={(e) => setNewTeamName(e.target.value)}
                autoFocus
              />
              <Button onClick={handleCreate} disabled={creating || !newTeamName.trim()}>
                {creating ? "Creating..." : "Create"}
              </Button>
            </div>
          </DialogContent>
        </Dialog>
      </div>

      {loading && <p className="text-muted-foreground">Loading teams...</p>}

      <div className="flex flex-col gap-2">
        {teams.map((t) => (
          <Link key={t.id} href={`/teams/${t.id}`}>
            <Card className="transition-colors hover:bg-accent">
              <CardHeader className="py-4">
                <CardTitle className="text-base">{t.name}</CardTitle>
              </CardHeader>
            </Card>
          </Link>
        ))}
        {!loading && teams.length === 0 && (
          <p className="py-8 text-center text-muted-foreground">
            No teams yet. Create one to get started.
          </p>
        )}
      </div>
    </div>
  );
}
