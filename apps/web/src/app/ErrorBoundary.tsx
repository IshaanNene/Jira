import { Component, type ErrorInfo, type ReactNode } from "react";

import { Button } from "@/components/ui";

interface Props {
  children: ReactNode;
  /** Changing this value clears the error (e.g. the current route path). */
  resetKey?: string;
}

interface State {
  error: Error | null;
}

/** Contains rendering failures to the region they happened in instead of blanking the app. */
export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("Unhandled render error", error, info.componentStack);
  }

  override componentDidUpdate(previous: Props): void {
    if (this.state.error && previous.resetKey !== this.props.resetKey) this.setState({ error: null });
  }

  override render(): ReactNode {
    if (!this.state.error) return this.props.children;
    return (
      <div role="alert" className="flex h-full min-h-[16rem] flex-col items-center justify-center gap-3 px-6 text-center">
        <h2 className="text-lg font-semibold text-ink">This page hit a problem</h2>
        <p className="max-w-sm text-sm text-ink-muted">Something unexpected went wrong while showing this page. Your data is safe.</p>
        <div className="flex gap-2">
          <Button onClick={() => this.setState({ error: null })}>Try again</Button>
          <Button variant="ghost" onClick={() => window.location.assign("/")}>Go home</Button>
        </div>
      </div>
    );
  }
}
