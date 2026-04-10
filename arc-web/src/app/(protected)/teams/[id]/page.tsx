"use client";

import { useState, useEffect } from "react";
import { useParams, useRouter } from "next/navigation";
import { apiClient } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import type { MemberResponse } from "@/types/api";

export default function TeamDetailPage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();
  const [members, setMembers] = useState<MemberResponse[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    apiClient
      .get<MemberResponse[]>(`/api/teams/${id}/members`)
      .then(setMembers)
      .finally(() => setLoading(false));
  }, [id]);

  const roleBadgeVariant = (role: string) => {
    switch (role) {
      case "owner":
        return "default" as const;
      case "admin":
        return "secondary" as const;
      default:
        return "outline" as const;
    }
  };

  return (
    <div className="mx-auto max-w-3xl">
      <div className="mb-6 flex items-center gap-3">
        <Button variant="ghost" size="sm" onClick={() => router.push("/teams")}>
          &larr; Teams
        </Button>
        <h1 className="text-2xl font-semibold">Team Members</h1>
      </div>

      {loading && <p className="text-muted-foreground">Loading members...</p>}

      <div className="flex flex-col gap-2">
        {members.map((m) => (
          <div
            key={m.user_id}
            className="flex items-center justify-between rounded border p-4"
          >
            <span className="font-mono text-sm">{m.user_id}</span>
            <Badge variant={roleBadgeVariant(m.role)}>{m.role}</Badge>
          </div>
        ))}
        {!loading && members.length === 0 && (
          <p className="py-8 text-center text-muted-foreground">No members</p>
        )}
      </div>
    </div>
  );
}
