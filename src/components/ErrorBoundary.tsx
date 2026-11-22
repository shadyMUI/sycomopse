import { Component, type ErrorInfo, type ReactNode } from "react";

export interface ErrorBoundaryProps {
  children: ReactNode;
  fallbackTitle?: string;
  onReset?: () => void;
}

export interface ErrorBoundaryState {
  hasError: boolean;
  error: Error | null;
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  public state: ErrorBoundaryState = {
    hasError: false,
    error: null,
  };

  public static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error };
  }

  public componentDidCatch(error: Error, errorInfo: ErrorInfo): void {
    console.error("ErrorBoundary caught an unhandled render error:", error, errorInfo);
  }

  public reset = (): void => {
    this.props.onReset?.();
    this.setState({ hasError: false, error: null });
  };

  public render(): ReactNode {
    if (this.state.hasError) {
      return (
        <section className="error-fallback panel" role="alert">
          <div className="section-heading">
            <div>
              <span className="eyebrow">Interface recovery</span>
              <h2>{this.props.fallbackTitle ?? "An unexpected error occurred"}</h2>
            </div>
            <span className="status-pill muted">Interrupted</span>
          </div>
          <p className="control-reason">
            {this.state.error?.message ?? "Unknown error occurred while rendering the component."}
          </p>
          <div className="error-fallback-actions" style={{ display: "flex", gap: "10px", marginTop: "16px" }}>
            <button
              type="button"
              className="button button-primary button-small"
              onClick={this.reset}
            >
              Retry
            </button>
            <button
              type="button"
              className="button button-secondary button-small"
              onClick={() => window.location.reload()}
            >
              Reload application
            </button>
          </div>
        </section>
      );
    }
    return this.props.children;
  }
}
