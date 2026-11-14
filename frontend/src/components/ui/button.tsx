import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-[10px] text-sm font-semibold transition-colors focus-visible:outline-2 focus-visible:outline-dashed focus-visible:outline-[var(--accent)] disabled:pointer-events-none disabled:opacity-45 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        default:
          "bg-[var(--accent)] text-[var(--on-accent)] hover:bg-[var(--accent-hover)]",
        secondary:
          "bg-[var(--accent-soft)] text-[var(--accent-deep)] hover:bg-[var(--nav-hover-bg)]",
        outline:
          "border border-[var(--line)] bg-[var(--card)] text-[var(--ink)] hover:bg-[var(--accent-soft)]",
        ghost:
          "text-[var(--accent-deep)] hover:bg-[var(--accent-soft)]",
        destructive:
          "bg-[var(--danger)] text-[var(--on-accent)] hover:opacity-90",
      },
      size: {
        default: "h-10 px-5 py-2",
        sm: "h-8 px-3 text-[13px]",
        lg: "h-12 px-6",
        icon: "h-10 w-10",
      },
    },
    defaultVariants: { variant: "default", size: "default" },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";
    return (
      <Comp
        className={cn(buttonVariants({ variant, size, className }))}
        ref={ref}
        {...props}
      />
    );
  },
);
Button.displayName = "Button";

export { Button, buttonVariants };
