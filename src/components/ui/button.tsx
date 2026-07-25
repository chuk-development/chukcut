import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import type * as React from "react";

import { cn } from "@/lib/utils";

/**
 * Chrome in this app is dense and small. The default size is 26px tall with
 * 12px text, which is what the header, toolbars and inspector use; `md` exists
 * for dialogs, where a button is the thing you are looking at rather than a
 * control you are reaching past.
 */
const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-sm text-[12px] font-medium leading-none transition-colors outline-none focus-visible:ring-[2px] focus-visible:ring-ring/60 disabled:pointer-events-none disabled:opacity-40 [&_svg]:pointer-events-none [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-primary/90",
        secondary: "bg-secondary text-secondary-foreground hover:bg-accent",
        outline: "border border-border bg-transparent hover:bg-accent hover:text-accent-foreground",
        ghost: "text-panel-foreground hover:bg-accent hover:text-accent-foreground",
        destructive: "bg-destructive text-destructive-foreground hover:bg-destructive/90",
        toggle:
          "text-muted-foreground hover:bg-accent hover:text-accent-foreground data-[active=true]:bg-accent data-[active=true]:text-foreground",
      },
      size: {
        default: "h-[26px] px-2.5 [&_svg]:size-3.5",
        sm: "h-[22px] px-2 text-[11px] [&_svg]:size-3",
        md: "h-8 px-3.5 text-[13px] [&_svg]:size-4",
        icon: "size-[26px] [&_svg]:size-3.5",
        "icon-sm": "size-[22px] [&_svg]:size-3",
        "icon-lg": "size-8 [&_svg]:size-[18px]",
      },
    },
    defaultVariants: { variant: "ghost", size: "default" },
  },
);

function Button({
  className,
  variant,
  size,
  asChild = false,
  ...props
}: React.ComponentProps<"button"> & VariantProps<typeof buttonVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot : "button";
  return (
    <Comp
      data-slot="button"
      className={cn(buttonVariants({ variant, size }), className)}
      {...props}
    />
  );
}

export { Button, buttonVariants };
