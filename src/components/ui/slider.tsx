import * as SliderPrimitive from "@radix-ui/react-slider";
import type * as React from "react";

import { cn } from "@/lib/utils";

function Slider({ className, ...props }: React.ComponentProps<typeof SliderPrimitive.Root>) {
  return (
    <SliderPrimitive.Root
      data-slot="slider"
      className={cn(
        "relative flex h-4 w-full touch-none select-none items-center data-[disabled]:opacity-40",
        className,
      )}
      {...props}
    >
      <SliderPrimitive.Track
        data-slot="slider-track"
        className="relative h-[3px] w-full grow overflow-hidden rounded-full bg-input"
      >
        <SliderPrimitive.Range data-slot="slider-range" className="absolute h-full bg-primary" />
      </SliderPrimitive.Track>
      <SliderPrimitive.Thumb
        data-slot="slider-thumb"
        className="block size-3 rounded-full border-2 border-primary bg-foreground outline-none transition-[box-shadow] focus-visible:ring-[3px] focus-visible:ring-ring/50"
      />
    </SliderPrimitive.Root>
  );
}

export { Slider };
