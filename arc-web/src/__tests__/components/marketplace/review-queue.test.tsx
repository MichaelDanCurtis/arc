import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import AdminReviewPage from "@/app/marketplace/admin/review/page";

vi.mock("@/lib/api", () => ({
  apiClient: {
    get: vi.fn().mockResolvedValue([]),
    post: vi.fn(),
  },
}));

describe("AdminReviewPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders page title", async () => {
    render(<AdminReviewPage />);
    expect(screen.getByText("Review Queue")).toBeInTheDocument();
  });
});
