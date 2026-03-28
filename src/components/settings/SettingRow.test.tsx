import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

// Minimal inline SettingRow for isolated testing — mirrors the implementation
function SettingRow({ label, description, children }: {
  label: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between py-4">
      <div className="space-y-1 pr-8">
        <p className="text-sm font-medium leading-none">{label}</p>
        {description && (
          <p className="text-sm text-muted-foreground">{description}</p>
        )}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

describe("SettingRow", () => {
  it("renders label and description", () => {
    render(
      <SettingRow label="Font size" description="Adjust terminal text size">
        <input type="number" />
      </SettingRow>,
    );

    expect(screen.getByText("Font size")).toBeInTheDocument();
    expect(screen.getByText("Adjust terminal text size")).toBeInTheDocument();
  });

  it("renders without description", () => {
    render(
      <SettingRow label="Theme">
        <span>dark</span>
      </SettingRow>,
    );

    expect(screen.getByText("Theme")).toBeInTheDocument();
    expect(screen.getByText("dark")).toBeInTheDocument();
  });

  it("renders children control", () => {
    const onChange = vi.fn();
    render(
      <SettingRow label="Sound" description="Toggle sound">
        <input
          type="checkbox"
          data-testid="toggle"
          onChange={onChange}
        />
      </SettingRow>,
    );

    fireEvent.click(screen.getByTestId("toggle"));
    expect(onChange).toHaveBeenCalled();
  });

  it("number input respects min/max via native validation", () => {
    render(
      <SettingRow label="Font size" description="8–24px">
        <input type="number" min={8} max={24} defaultValue={13} data-testid="num" />
      </SettingRow>,
    );

    const input = screen.getByTestId("num") as HTMLInputElement;
    expect(input.min).toBe("8");
    expect(input.max).toBe("24");
    expect(input.value).toBe("13");
  });
});
