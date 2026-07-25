import * as TabsPrimitive from "@radix-ui/react-tabs";
import type * as React from "react";

import { cn } from "@/lib/utils";

function Tabs({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.Root>) {
  return (
    <TabsPrimitive.Root
      data-slot="tabs"
      className={cn("flex flex-col min-h-0", className)}
      {...props}
    />
  );
}

function TabsList({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.List>) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      className={cn("flex items-stretch gap-px", className)}
      {...props}
    />
  );
}

/**
 * Editor tabs read as a strip of destinations, not as pills: an underline on
 * the active one and no chrome anywhere else keeps the row quiet enough to sit
 * directly under the header.
 */
function TabsTrigger({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      className={cn(
        "relative flex flex-col items-center justify-center gap-1 px-2.5 py-1.5 text-[11px] font-medium text-muted-foreground outline-none transition-colors",
        "hover:text-foreground focus-visible:ring-[2px] focus-visible:ring-ring/60",
        "data-[state=active]:text-foreground",
        "after:absolute after:inset-x-1.5 after:-bottom-px after:h-[2px] after:bg-transparent data-[state=active]:after:bg-primary",
        "[&_svg]:size-4 [&_svg]:shrink-0",
        className,
      )}
      {...props}
    />
  );
}

function TabsContent({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.Content>) {
  return (
    <TabsPrimitive.Content
      data-slot="tabs-content"
      className={cn("flex-1 min-h-0 outline-none", className)}
      {...props}
    />
  );
}

export { Tabs, TabsContent, TabsList, TabsTrigger };
