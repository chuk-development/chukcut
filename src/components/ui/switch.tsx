import * as SwitchPrimitive from "@radix-ui/react-switch";
import type * as React from "react";

import { cn } from "@/lib/utils";

/**
 * Sized to sit on the same 26px row as `Input` and `SelectTrigger`, so a
 * settings grid does not develop a taller line wherever a toggle appears.
 */
function Switch({ className, ...props }: React.ComponentProps<typeof SwitchPrimitive.Root>) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        "peer inline-flex h-[18px] w-[32px] shrink-0 items-center rounded-full border border-transparent p-px outline-none transition-colors",
        "bg-input data-[state=checked]:bg-primary",
        "focus-visible:ring-[2px] focus-visible:ring-ring/60",
        "disabled:pointer-events-none disabled:opacity-40",
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className={cn(
          "pointer-events-none block size-[14px] rounded-full bg-foreground shadow-sm transition-transform",
          "data-[state=unchecked]:translate-x-0 data-[state=checked]:translate-x-[14px]",
        )}
      />
    </SwitchPrimitive.Root>
  );
}

export { Switch };
