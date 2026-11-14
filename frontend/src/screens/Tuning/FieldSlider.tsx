import * as SliderPrimitive from "@radix-ui/react-slider";
import { cn } from "@/lib/utils";

/**
 * Labelled slider (Radix primitive, same styling as the scaffold slider).
 *
 * The scaffold `ui/slider` hard-codes its thumb's accessible name to
 * "Value", which would leave five sliders in one panel indistinguishable
 * to a screen reader (P10.2: every control needs its own accessible
 * name). This local slider puts the field label on the thumb itself, so
 * `getByRole("slider", { name: "Deadzone" })` works and a screen reader
 * announces the field. Styling matches the scaffold component exactly.
 */

export interface FieldSliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  disabled?: boolean;
  /** Formatted value shown next to the label, e.g. "5%" or "1200 mV". */
  valueText: string;
  onChange: (value: number) => void;
}

export function FieldSlider({
  label,
  value,
  min,
  max,
  step,
  disabled = false,
  valueText,
  onChange,
}: FieldSliderProps) {
  return (
    <div role="group" aria-label={label} className="space-y-2">
      <div className="flex items-baseline justify-between gap-2">
        <span className="text-sm font-medium text-[var(--ink)]">{label}</span>
        <span className="text-sm font-semibold tabular-nums text-[var(--accent-deep)]">
          {valueText}
        </span>
      </div>
      <SliderPrimitive.Root
        className={cn(
          "relative flex w-full touch-none select-none items-center",
          disabled && "opacity-50",
        )}
        value={[value]}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        onValueChange={(nextValues) => {
          const next = nextValues[0];
          if (next !== undefined) onChange(next);
        }}
      >
        <SliderPrimitive.Track className="relative h-2 w-full grow overflow-hidden rounded-full bg-[var(--track)]">
          <SliderPrimitive.Range className="absolute h-full bg-[var(--accent)]" />
        </SliderPrimitive.Track>
        <SliderPrimitive.Thumb
          aria-label={label}
          // Radix marks the thumb disabled only via data-disabled; expose
          // it as aria-disabled so the state is perceivable (P10.2).
          aria-disabled={disabled || undefined}
          className="block h-5 w-5 rounded-full border-2 border-[var(--accent)] bg-[var(--card)] shadow-md transition-colors focus-visible:outline-2 focus-visible:outline-dashed focus-visible:outline-[var(--accent)] disabled:pointer-events-none disabled:opacity-50"
        />
      </SliderPrimitive.Root>
    </div>
  );
}
