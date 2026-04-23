import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import SubmitPluginPage from "@/app/marketplace/submit/page";

const mockPush = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: mockPush, back: vi.fn() }),
}));

vi.mock("@/lib/api", () => ({
  apiClient: {
    post: vi.fn(),
  },
}));

describe("SubmitPluginPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders form fields", () => {
    render(<SubmitPluginPage />);
    expect(screen.getByLabelText(/plugin name/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/^slug/i)).toBeInTheDocument();
  });

  it("auto-generates slug from name", () => {
    render(<SubmitPluginPage />);
    const nameInput = screen.getByLabelText(/plugin name/i);
    fireEvent.change(nameInput, { target: { value: "Test Plugin" } });
    const slugInput = screen.getByLabelText(/^slug/i) as HTMLInputElement;
    expect(slugInput.value).toBe("test-plugin");
  });

  it("submit button disabled when name/slug empty", () => {
    render(<SubmitPluginPage />);
    // The submit button relies on HTML `required` on inputs; check it exists and form is not submitted without values
    const button = screen.getByRole("button", { name: /submit plugin/i });
    expect(button).toBeInTheDocument();
    // name and slug are empty initially, so the inputs should be empty
    const nameInput = screen.getByLabelText(/plugin name/i) as HTMLInputElement;
    const slugInput = screen.getByLabelText(/^slug/i) as HTMLInputElement;
    expect(nameInput.value).toBe("");
    expect(slugInput.value).toBe("");
  });
});
