import "./styles/index.css";

import { QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider } from "react-router-dom";

import { Toaster, TooltipProvider } from "@/components/ui";
import { bootstrapSession } from "@/modules/auth";
import { queryClient } from "@/services/query-client";
import { initTheme } from "@/state/ui";

import { router } from "./app/router";

initTheme();
// Restore the session from the HttpOnly refresh cookie; routes wait on the result.
void bootstrapSession();

const container = document.getElementById("root");
if (!container) throw new Error("Root element is missing from index.html");

createRoot(container).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <TooltipProvider delayDuration={300}>
        <RouterProvider router={router} />
        <Toaster />
      </TooltipProvider>
    </QueryClientProvider>
  </StrictMode>,
);
