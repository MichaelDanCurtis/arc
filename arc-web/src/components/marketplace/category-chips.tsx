"use client";

interface CategoryChipsProps {
  categories: string[];
  selected: string | null;
  onSelect: (category: string | null) => void;
}

export function CategoryChips({ categories, selected, onSelect }: CategoryChipsProps) {
  return (
    <div className="flex flex-wrap gap-2">
      <button
        onClick={() => onSelect(null)}
        className={`rounded-full border px-4 py-1 text-sm transition-colors ${
          selected === null
            ? "border-primary bg-primary text-primary-foreground font-semibold"
            : "border-border bg-card text-muted-foreground hover:text-foreground"
        }`}
      >
        All
      </button>
      {categories.map((cat) => (
        <button
          key={cat}
          onClick={() => onSelect(cat === selected ? null : cat)}
          className={`rounded-full border px-4 py-1 text-sm capitalize transition-colors ${
            cat === selected
              ? "border-primary bg-primary text-primary-foreground font-semibold"
              : "border-border bg-card text-muted-foreground hover:text-foreground"
          }`}
        >
          {cat}
        </button>
      ))}
    </div>
  );
}
