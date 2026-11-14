import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold transition-colors focus:outline-2 focus:outline-dashed focus:outline-[var(--accent)]",
  {
    variants: {
      variant: {
        default:
          "border-transparent bg-[var(--accent)] text-[var(--on-accent)]",
        secondary:
          "border-transparent bg-[var(--accent-soft)] text-[var(--accent-deep)]",
        success:
          "border-transparent bg-[var(--success-pill)] text-[var(--success-text)]",
        destructive:
          "border-transparent bg-[var(--danger-pill)] text-[var(--error-text)]",
        outline: "border-[var(--line)] text-[var(--ink)]",
      },
    },
    defaultVariants: { variant: "default" },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLDivElement>,
    VariantProps<typeof badgeVariants> {}

function Badge({ className, variant, ...props }: BadgeProps) {
  return (
    <div className={cn(badgeVariants({ variant }), className)} {...props} />
  );
}

export { Badge, badgeVariants };
