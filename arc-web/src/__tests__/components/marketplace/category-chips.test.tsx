import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CategoryChips } from "@/components/marketplace/category-chips";

const categories = ["ai", "devops", "git"];

describe("CategoryChips", () => {
  it("renders All button and all category buttons", () => {
    render(<CategoryChips categories={categories} selected={null} onSelect={vi.fn()} />);
    expect(screen.getByText("All")).toBeInTheDocument();
    expect(screen.getByText("ai")).toBeInTheDocument();
    expect(screen.getByText("devops")).toBeInTheDocument();
    expect(screen.getByText("git")).toBeInTheDocument();
  });

  it("calls onSelect with category when clicking a category", () => {
    const onSelect = vi.fn();
    render(<CategoryChips categories={categories} selected={null} onSelect={onSelect} />);
    fireEvent.click(screen.getByText("ai"));
    expect(onSelect).toHaveBeenCalledWith("ai");
  });

  it("calls onSelect with null when clicking All", () => {
    const onSelect = vi.fn();
    render(<CategoryChips categories={categories} selected="ai" onSelect={onSelect} />);
    fireEvent.click(screen.getByText("All"));
    expect(onSelect).toHaveBeenCalledWith(null);
  });

  it("deselects category when clicking already-selected category", () => {
    const onSelect = vi.fn();
    render(<CategoryChips categories={categories} selected="ai" onSelect={onSelect} />);
    fireEvent.click(screen.getByText("ai"));
    expect(onSelect).toHaveBeenCalledWith(null);
  });
});
