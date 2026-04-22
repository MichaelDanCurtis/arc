"use client";

import { useState } from "react";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";

interface SearchBarProps {
  onSearch: (query: string) => void;
  initial?: string;
}

export function SearchBar({ onSearch, initial = "" }: SearchBarProps) {
  const [value, setValue] = useState(initial);

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    onSearch(value);
  }

  return (
    <form onSubmit={handleSubmit} className="flex gap-2">
      <Input
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder="Search plugins..."
        className="max-w-lg"
      />
      <Button type="submit">Search</Button>
    </form>
  );
}
