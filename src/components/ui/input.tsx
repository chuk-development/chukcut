import type * as React from "react";

import { cn } from "@/lib/utils";

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "h-[26px] w-full min-w-0 rounded-sm border border-input bg-background px-2 text-[12px] text-foreground",
        "placeholder:text-muted-foreground outline-none transition-[border-color,box-shadow]",
        "focus-visible:border-ring focus-visible:ring-[2px] focus-visible:ring-ring/40",
        "disabled:pointer-events-none disabled:opacity-40",
        "[&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none",
        className,
      )}
      {...props}
    />
  );
}

export { Input };
