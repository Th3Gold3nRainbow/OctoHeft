import React from 'react';
import ReactDOM from 'react-dom/client';
import { AlertCircle, RotateCw } from 'lucide-react';
import App from './App';
import { I18nProvider } from './i18n';
import './index.css';

interface ErrorBoundaryState {
  hasError: boolean;
  errorMessage: string;
}

class ErrorBoundary extends React.Component<{ children: React.ReactNode }, ErrorBoundaryState> {
  constructor(props: { children: React.ReactNode }) {
    super(props);
    this.state = {
      hasError: false,
      errorMessage: '',
    };
  }

  static getDerivedStateFromError(error: unknown): ErrorBoundaryState {
    const msg = error instanceof Error ? `${error.name}: ${error.message}` : String(error || 'Unknown error');
    return {
      hasError: true,
      errorMessage: msg,
    };
  }

  componentDidCatch(error: unknown, info: React.ErrorInfo): void {
    console.error('[OctoHeft UI ErrorBoundary]', error, info.componentStack);
  }

  render() {
    if (this.state.hasError) {
      return (
        <main className="bg-app min-h-screen w-screen bg-slate-50 dark:bg-slate-950 text-slate-900 dark:text-slate-100 flex items-center justify-center p-6 font-sans">
          <section
            aria-labelledby="error-boundary-heading"
            className="card-surface w-full max-w-lg rounded-2xl border border-slate-300 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 shadow-lg space-y-4"
          >
            <div className="flex items-start gap-3 rounded-xl border border-rose-300 dark:border-rose-500/40 bg-rose-50 dark:bg-rose-950/60 p-4 text-rose-900 dark:text-rose-200">
              <AlertCircle className="h-5 w-5 text-rose-700 dark:text-rose-400 shrink-0 mt-0.5" aria-hidden="true" />
              <div className="min-w-0 space-y-1">
                <p className="text-xs font-bold uppercase tracking-wider text-rose-800 dark:text-rose-300">
                  Render Recovery
                </p>
                <h1 id="error-boundary-heading" className="text-base font-bold text-slate-900 dark:text-slate-100">
                  Something went wrong while rendering OctoHeft
                </h1>
                <p className="text-xs text-slate-700 dark:text-slate-300 leading-relaxed">
                  The user interface encountered an unexpected error. Your download queue on disk is safe.
                </p>
              </div>
            </div>

            {this.state.errorMessage && (
              <pre
                dir="ltr"
                className="octoheft-subcard rounded-xl border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-800 p-3 text-[11px] font-mono text-slate-800 dark:text-slate-200 whitespace-pre-wrap break-words max-h-40 overflow-y-auto select-text"
              >
                {this.state.errorMessage}
              </pre>
            )}

            <div className="flex justify-end">
              <button
                type="button"
                onClick={() => window.location.reload()}
                className="inline-flex items-center gap-2 rounded-lg bg-sky-700 px-4 py-2 text-xs font-semibold text-white hover:bg-sky-800 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-sky-700"
              >
                <RotateCw className="h-4 w-4" aria-hidden="true" />
                <span>Reload Application</span>
              </button>
            </div>
          </section>
        </main>
      );
    }

    return this.props.children;
  }
}

window.addEventListener('error', (event) => {
  console.error('[OctoHeft Global Error]', event.error || event.message);
});

window.addEventListener('unhandledrejection', (event) => {
  console.error('[OctoHeft Unhandled Promise Rejection]', event.reason);
});

const rootEl = document.getElementById('root');
if (rootEl) {
  ReactDOM.createRoot(rootEl).render(
    <React.StrictMode>
      <ErrorBoundary>
        <I18nProvider>
          <App />
        </I18nProvider>
      </ErrorBoundary>
    </React.StrictMode>
  );
}
