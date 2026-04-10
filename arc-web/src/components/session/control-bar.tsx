"use client";

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

interface ControlRequest {
  userId: string;
  userName: string;
}

interface Props {
  isController: boolean;
  controllerName: string;
  pendingRequests: ControlRequest[];
  onRequestControl: () => void;
  onGrantControl: (userId: string) => void;
  onDenyControl: (userId: string) => void;
}

export function ControlBar({
  isController,
  controllerName,
  pendingRequests,
  onRequestControl,
  onGrantControl,
  onDenyControl,
}: Props) {
  const [dialogOpen, setDialogOpen] = useState(false);

  return (
    <>
      <div className="flex items-center gap-2">
        <Badge variant={isController ? "default" : "secondary"}>
          {isController ? "Controlling" : "Watching"}
        </Badge>

        <span className="text-sm text-muted-foreground">
          Controller: {controllerName}
        </span>

        {!isController && (
          <Button variant="outline" size="sm" onClick={onRequestControl}>
            Request Control
          </Button>
        )}

        {isController && pendingRequests.length > 0 && (
          <Button
            variant="outline"
            size="sm"
            onClick={() => setDialogOpen(true)}
          >
            {pendingRequests.length} request{pendingRequests.length > 1 ? "s" : ""}
          </Button>
        )}
      </div>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Control Requests</DialogTitle>
          </DialogHeader>
          <div className="flex flex-col gap-3">
            {pendingRequests.map((req) => (
              <div key={req.userId} className="flex items-center justify-between">
                <span className="text-sm">{req.userName}</span>
                <div className="flex gap-2">
                  <Button
                    size="sm"
                    onClick={() => {
                      onGrantControl(req.userId);
                      setDialogOpen(false);
                    }}
                  >
                    Accept
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => onDenyControl(req.userId)}
                  >
                    Deny
                  </Button>
                </div>
              </div>
            ))}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
