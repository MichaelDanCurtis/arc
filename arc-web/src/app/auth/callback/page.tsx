import { Suspense } from "react";
import { OAuthCallbackInner } from "./callback-inner";

export default function OAuthCallbackPage() {
  return (
    <Suspense
      fallback={
        <div className="flex h-screen items-center justify-center">
          <p className="text-muted-foreground">Signing in...</p>
        </div>
      }
    >
      <OAuthCallbackInner />
    </Suspense>
  );
}
