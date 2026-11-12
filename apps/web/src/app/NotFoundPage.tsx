import { Compass } from "lucide-react";
import { Link } from "react-router-dom";

import { Button, EmptyState } from "@/components/ui";

export function NotFoundPage() {
  return (
    <EmptyState
      className="py-24"
      icon={Compass}
      title="Page not found"
      description="The address may be mistyped, or the page may have moved."
      action={<Button asChild variant="primary"><Link to="/">Go home</Link></Button>}
    />
  );
}
