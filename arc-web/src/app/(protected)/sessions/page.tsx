"use client";

import { useState } from "react";
import { useSessions } from "@/hooks/use-session";
import { SessionCard } from "@/components/session/session-card";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Button } from "@/components/ui/button";

type StateFilter = "all" | "active" | "suspended" | "ended";

export default function SessionsPage() {
  const { sessions, loading, error } = useSessions();
  const [filter, setFilter] = useState<StateFilter>("all");
  const [search, setSearch] = useState("");

  const filtered = sessions.filter((s) => {
    if (filter !== "all" && s.state !== filter) return false;
    if (search && !(s.name ?? s.id).toLowerCase().includes(search.toLowerCase()))
      return false;
    return true;
  });

  return (
    <div className="mx-auto max-w-3xl">
      <div className="mb-6 flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Sessions</h1>
        <div className="flex gap-2">
          <Input
            placeholder="Search sessions..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="w-48"
          />
          <DropdownMenu>
            <DropdownMenuTrigger className="inline-flex items-center justify-center rounded-md border border-input bg-background px-3 py-2 text-sm font-medium shadow-sm hover:bg-accent hover:text-accent-foreground">
              {filter === "all" ? "All states" : filter}
            </DropdownMenuTrigger>
            <DropdownMenuContent>
              <DropdownMenuItem onClick={() => setFilter("all")}>All</DropdownMenuItem>
              <DropdownMenuItem onClick={() => setFilter("active")}>Active</DropdownMenuItem>
              <DropdownMenuItem onClick={() => setFilter("suspended")}>Suspended</DropdownMenuItem>
              <DropdownMenuItem onClick={() => setFilter("ended")}>Ended</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>

      {loading && <p className="text-muted-foreground">Loading sessions...</p>}
      {error && <p className="text-destructive">{error}</p>}

      <div className="flex flex-col gap-2">
        {filtered.map((s) => (
          <SessionCard key={s.id} session={s} />
        ))}
        {!loading && filtered.length === 0 && (
          <p className="py-8 text-center text-muted-foreground">
            No sessions found
          </p>
        )}
      </div>
    </div>
  );
}
