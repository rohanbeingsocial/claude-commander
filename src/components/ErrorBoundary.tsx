import { Component, type ErrorInfo, type ReactNode } from "react";

interface State {
  error: Error | null;
  info: string;
}

/** Catches render errors so a crash shows a readable message instead of a blank window. */
export default class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null, info: "" };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // eslint-disable-next-line no-console
    console.error("Render error:", error, info.componentStack);
    this.setState({ info: info.componentStack ?? "" });
  }

  render() {
    const { error, info } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="crash-screen">
        <h1>Something crashed the UI</h1>
        <p className="dim">Reloading (Ctrl+R) usually recovers. Details below:</p>
        <pre className="crash-detail">
          {String(error.stack || error.message)}
          {info ? `\n\nComponent stack:${info}` : ""}
        </pre>
        <button className="btn btn-primary" onClick={() => location.reload()}>
          Reload
        </button>
      </div>
    );
  }
}
