"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { getAccessToken } from "@/lib/auth";
import { getRelayUrl } from "@/lib/config";
import type { SetupStatus } from "@/types/api";

export default function Home() {
  const router = useRouter();

  useEffect(() => {
    // First, check if setup has been completed. If not, route to the wizard.
    // Otherwise fall back to the normal auth-gated flow.
    const token = getAccessToken();
    fetch(`${getRelayUrl()}/api/setup/status`)
      .then((r) => (r.ok ? (r.json() as Promise<SetupStatus>) : null))
      .then((status) => {
        if (status && !status.is_configured) {
          router.replace("/setup");
        } else {
          router.replace(token ? "/sessions" : "/login");
        }
      })
      .catch(() => {
        // If status check fails, fall through to the existing auth flow.
        router.replace(token ? "/sessions" : "/login");
      });
  }, [router]);

  return null;
}
