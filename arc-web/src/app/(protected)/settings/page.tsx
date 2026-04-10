"use client";

import { useAuth } from "@/hooks/use-auth";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";

export default function SettingsPage() {
  const { user } = useAuth();

  return (
    <div className="mx-auto max-w-xl">
      <h1 className="mb-6 text-2xl font-semibold">Settings</h1>

      <Card>
        <CardHeader>
          <CardTitle>Profile</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div>
            <Label className="text-muted-foreground">Email</Label>
            <p className="font-mono">{user?.email}</p>
          </div>
          <Separator />
          <div>
            <Label className="text-muted-foreground">Name</Label>
            <p>{user?.name ?? "Not set"}</p>
          </div>
          <Separator />
          <div>
            <Label className="text-muted-foreground">User ID</Label>
            <p className="font-mono text-sm text-muted-foreground">
              {user?.id}
            </p>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
