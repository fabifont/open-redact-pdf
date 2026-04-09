import type {
  ApplyReport,
  QuadGroupTarget,
  RectTarget,
  RedactionMode,
} from "@open-redact-pdf/sdk";

type UiTextMatch = {
  text: string;
  pageIndex: number;
};

type SidebarProps = {
  hasHandle: boolean;
  searchQuery: string;
  onSearchQueryChange: (value: string) => void;
  onSearch: () => void;
  searchMatches: UiTextMatch[];
  manualTargets: RectTarget[];
  searchTargets: QuadGroupTarget[];
  redactionMode: RedactionMode;
  onRedactionModeChange: (mode: RedactionMode) => void;
  onApply: () => void;
  onClear: () => void;
  onDownload: () => void;
  applyReport: ApplyReport | null;
  downloadReady: boolean;
  pageTexts: Array<{ text: string; error: string | null }>;
};

export function Sidebar({
  hasHandle,
  searchQuery,
  onSearchQueryChange,
  onSearch,
  searchMatches,
  manualTargets,
  searchTargets,
  redactionMode,
  onRedactionModeChange,
  onApply,
  onClear,
  onDownload,
  applyReport,
  downloadReady,
  pageTexts,
}: SidebarProps) {
  const targetCount = manualTargets.length + searchTargets.length;

  return (
    <aside className="sidebar">
      {/* Search */}
      <div className="sidebar-section">
        <div className="section-title">Search</div>
        <div className="input-row">
          <input
            className="input"
            value={searchQuery}
            onChange={(event) => onSearchQueryChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") onSearch();
            }}
            placeholder="Text to redact..."
          />
          <button
            type="button"
            className="btn btn-primary"
            onClick={onSearch}
            disabled={!hasHandle || !searchQuery.trim()}
          >
            Find
          </button>
        </div>
      </div>

      {/* Plan */}
      <div className="sidebar-section">
        <div className="section-title">Plan</div>
        <div className="stat-row">
          <span className="stat-pill">
            <strong>{manualTargets.length}</strong> rect
          </span>
          <span className="stat-pill">
            <strong>{searchTargets.length}</strong> search
          </span>
          <span className="stat-pill">
            <strong>{targetCount}</strong> total
          </span>
        </div>
        <div className="field">
          <label className="field-label">Mode</label>
          <select
            className="select"
            value={redactionMode}
            onChange={(event) =>
              onRedactionModeChange(event.target.value as RedactionMode)
            }
          >
            <option value="redact">Redact (black overlay)</option>
            <option value="erase">Erase (blank space)</option>
            <option value="strip">Strip (remove bytes)</option>
          </select>
        </div>
        <div className="button-row">
          <button
            type="button"
            className="btn btn-primary"
            onClick={onApply}
            disabled={!hasHandle || targetCount === 0}
          >
            Apply Redactions
          </button>
          <button
            type="button"
            className="btn btn-danger"
            onClick={onClear}
            disabled={targetCount === 0}
          >
            Clear
          </button>
        </div>
      </div>

      {/* Report */}
      {applyReport ? (
        <div className="sidebar-section">
          <div className="section-title">Report</div>
          <div className="report-row">
            <span className="report-label">Pages touched</span>
            <span className="report-value">{applyReport.pagesTouched}</span>
          </div>
          <div className="report-row">
            <span className="report-label">Glyphs removed</span>
            <span className="report-value">
              {applyReport.textGlyphsRemoved}
            </span>
          </div>
          <div className="report-row">
            <span className="report-label">Vector paints</span>
            <span className="report-value">
              {applyReport.pathPaintsRemoved}
            </span>
          </div>
          <div className="report-row">
            <span className="report-label">Image draws</span>
            <span className="report-value">
              {applyReport.imageDrawsRemoved}
            </span>
          </div>
          <div className="report-row">
            <span className="report-label">Annotations</span>
            <span className="report-value">
              {applyReport.annotationsRemoved}
            </span>
          </div>
          {applyReport.warnings.length > 0 ? (
            <div className="report-warnings">
              {applyReport.warnings.map((warning, index) => (
                <div key={index}>{warning}</div>
              ))}
            </div>
          ) : null}
          {downloadReady ? (
            <div className="button-row">
              <button
                type="button"
                className="btn btn-primary"
                onClick={onDownload}
              >
                Download PDF
              </button>
            </div>
          ) : null}
        </div>
      ) : null}

      {/* Search matches */}
      {searchMatches.length > 0 ? (
        <div className="sidebar-section">
          <div className="section-title">
            Matches ({searchMatches.length})
          </div>
          <ul className="match-list">
            {searchMatches.map((match, index) => (
              <li key={index} className="match-item">
                <span className="match-page">p{match.pageIndex + 1}</span>
                <span className="match-text">{match.text}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {/* Extracted text */}
      {pageTexts.length > 0 ? (
        <div className="sidebar-section">
          <div className="section-title">Extracted Text</div>
          {pageTexts.map((entry, index) => (
            <details key={index} className="text-panel">
              <summary>Page {index + 1}</summary>
              {entry.error ? (
                <div className="text-panel-content text-panel-error">
                  {entry.error}
                </div>
              ) : (
                <div className="text-panel-content">
                  {entry.text || "[empty]"}
                </div>
              )}
            </details>
          ))}
        </div>
      ) : null}
    </aside>
  );
}
