import { useCallback, useEffect, useState, type ChangeEvent, type FormEvent } from "react";
import {
  applyRedactions,
  extractText,
  freePdf,
  getPageCount,
  getPageSize,
  initWasm,
  openPdf,
  openPdfWithPassword,
  savePdf,
  searchText,
  type PdfHandle,
  type Point,
  type QuadGroupTarget,
  type RectTarget,
  type RedactionMode,
  type RedactionPlan,
  type ApplyReport,
} from "@fabifont/open-redact-pdf";
import { GlobalWorkerOptions, getDocument, type PDFDocumentProxy } from "pdfjs-dist";
import { Toolbar } from "./components/Toolbar";
import { Sidebar } from "./components/Sidebar";
import { PageView } from "./components/PageView";

GlobalWorkerOptions.workerSrc = new URL(
  "pdfjs-dist/build/pdf.worker.min.mjs",
  import.meta.url
).toString();

export const ZOOM_LEVELS = [0.5, 0.75, 1, 1.25, 1.5, 2] as const;

function isPasswordError(message: string): boolean {
  // The Rust `PdfError::InvalidPassword` surfaces as an error whose
  // Display string starts with "invalid password". Match case-
  // insensitively so future phrasing tweaks still route correctly.
  return /invalid password/i.test(message);
}

type UiTextMatch = {
  text: string;
  pageIndex: number;
  quads: Array<[Point, Point, Point, Point]>;
};

export function App() {
  const [status, setStatus] = useState("Load a PDF to start.");
  const [error, setError] = useState<string | null>(null);
  const [pdfBytes, setPdfBytes] = useState<Uint8Array | null>(null);
  const [handle, setHandle] = useState<PdfHandle | null>(null);
  const [pageSizes, setPageSizes] = useState<Array<{ width: number; height: number }>>([]);
  const [manualTargets, setManualTargets] = useState<RectTarget[]>([]);
  const [searchTargets, setSearchTargets] = useState<QuadGroupTarget[]>([]);
  const [searchMatches, setSearchMatches] = useState<UiTextMatch[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [downloadBytes, setDownloadBytes] = useState<Uint8Array | null>(null);
  const [applyReport, setApplyReport] = useState<ApplyReport | null>(null);
  const [redactionMode, setRedactionMode] = useState<RedactionMode>("redact");
  const [pageTexts, setPageTexts] = useState<Array<{ text: string; error: string | null }>>([]);
  const [renderErrors, setRenderErrors] = useState<Record<number, string>>({});
  const [previewDocument, setPreviewDocument] = useState<PDFDocumentProxy | null>(null);
  const [zoom, setZoom] = useState(1);
  const [collapsedPages, setCollapsedPages] = useState<Set<number>>(new Set());
  // When opening an encrypted PDF fails with PdfError::InvalidPassword we
  // stash the bytes here so the user can enter a password and retry.
  const [pendingEncryptedBytes, setPendingEncryptedBytes] = useState<Uint8Array | null>(null);
  const [passwordInput, setPasswordInput] = useState("");

  useEffect(() => {
    if (!pdfBytes) {
      setPreviewDocument(null);
      return;
    }
    let cancelled = false;
    let documentRef: PDFDocumentProxy | null = null;
    setRenderErrors({});
    getDocument({ data: Uint8Array.from(pdfBytes) })
      .promise.then((document) => {
        documentRef = document;
        if (!cancelled) setPreviewDocument(document);
      })
      .catch((caught) => {
        const message = caught instanceof Error ? caught.message : String(caught);
        if (!cancelled) {
          setError(message);
          setStatus("PDF.js preview failed.");
          setPreviewDocument(null);
        }
      });
    return () => {
      cancelled = true;
      setPreviewDocument((current) => (current === documentRef ? null : current));
      void documentRef?.destroy();
    };
  }, [pdfBytes]);

  async function loadPdfBytes(bytes: Uint8Array, password: string | null = null) {
    setError(null);
    setStatus("Initializing WebAssembly...");
    await initWasm();
    if (handle) {
      freePdf(handle);
      setHandle(null);
    }
    const input = Uint8Array.from(bytes);
    const nextHandle =
      password === null ? openPdf(input) : openPdfWithPassword(input, password);
    const pageCount = getPageCount(nextHandle);
    const sizes = Array.from({ length: pageCount }, (_, pageIndex) =>
      getPageSize(nextHandle, pageIndex)
    );
    setHandle(nextHandle);
    setPdfBytes(Uint8Array.from(bytes));
    setPageSizes(sizes);
    setRenderErrors({});
    setManualTargets([]);
    setSearchTargets([]);
    setSearchMatches([]);
    setApplyReport(null);
    setDownloadBytes(null);
    setCollapsedPages(new Set());
    const texts = Array.from({ length: pageCount }, (_, pageIndex) => {
      try {
        return { text: extractText(nextHandle, pageIndex).text, error: null };
      } catch (caught) {
        const message = caught instanceof Error ? caught.message : String(caught);
        return { text: "", error: message };
      }
    });
    setPageTexts(texts);
    const failureCount = texts.filter((entry) => entry.error !== null).length;
    if (failureCount > 0) {
      setStatus(
        `Loaded ${pageCount} page${pageCount === 1 ? "" : "s"}; text extraction unsupported on ${failureCount} page${failureCount === 1 ? "" : "s"}.`
      );
    } else {
      setStatus(`Loaded ${pageCount} page${pageCount === 1 ? "" : "s"}.`);
    }
  }

  async function onFileChange(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    if (!file) return;
    let bytes: Uint8Array;
    try {
      bytes = new Uint8Array(await file.arrayBuffer());
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      setError(message);
      setStatus("Failed to read file.");
      return;
    }
    try {
      await loadPdfBytes(bytes);
      setPendingEncryptedBytes(null);
      setPasswordInput("");
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      if (isPasswordError(message)) {
        setPendingEncryptedBytes(bytes);
        setPasswordInput("");
        setStatus("Enter the password to open this encrypted PDF.");
        setError(null);
      } else {
        setError(message);
        setStatus("Failed to load PDF.");
      }
    }
  }

  async function submitPasswordAttempt(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!pendingEncryptedBytes) return;
    try {
      await loadPdfBytes(pendingEncryptedBytes, passwordInput);
      setPendingEncryptedBytes(null);
      setPasswordInput("");
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      if (isPasswordError(message)) {
        setError("Password did not authenticate. Try again.");
      } else {
        setError(message);
        setStatus("Failed to load PDF.");
        setPendingEncryptedBytes(null);
        setPasswordInput("");
      }
    }
  }

  function cancelPasswordPrompt() {
    setPendingEncryptedBytes(null);
    setPasswordInput("");
    setStatus("Load a PDF to start.");
    setError(null);
  }

  function addManualTarget(target: RectTarget) {
    setManualTargets((current) => [...current, target]);
  }

  function clearTargets() {
    setManualTargets([]);
    setSearchTargets([]);
    setSearchMatches([]);
    setApplyReport(null);
  }

  function togglePageCollapsed(pageIndex: number) {
    setCollapsedPages((current) => {
      const next = new Set(current);
      if (next.has(pageIndex)) next.delete(pageIndex);
      else next.add(pageIndex);
      return next;
    });
  }

  function runSearch() {
    if (!handle || !searchQuery.trim()) return;
    try {
      const matches: UiTextMatch[] = [];
      const failures: string[] = [];
      for (let pageIndex = 0; pageIndex < pageSizes.length; pageIndex++) {
        try {
          for (const match of searchText(handle, pageIndex, searchQuery)) {
            const normalized = normalizeSearchMatch(match);
            if (normalized) {
              matches.push(normalized);
            } else {
              failures.push(`Page ${pageIndex + 1}: search result geometry was invalid`);
            }
          }
        } catch (caught) {
          const message = caught instanceof Error ? caught.message : String(caught);
          failures.push(`Page ${pageIndex + 1}: ${message}`);
        }
      }
      const targets = matches.map<QuadGroupTarget>((match) => ({
        kind: "quadGroup",
        pageIndex: match.pageIndex,
        quads: match.quads,
      }));
      setSearchMatches(matches);
      setSearchTargets(targets);
      setError(failures.length > 0 ? failures.join(" | ") : null);
      if (matches.length === 0 && failures.length > 0) {
        setStatus("Search unavailable on one or more pages.");
      } else {
        setStatus(`Found ${matches.length} match${matches.length === 1 ? "" : "es"}.`);
      }
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      setError(message);
      setStatus("Search failed.");
    }
  }

  async function applyPlan() {
    if (!handle || !pdfBytes) return;
    const plan: RedactionPlan = {
      targets: [...manualTargets, ...searchTargets],
      mode: redactionMode,
      removeIntersectingAnnotations: true,
      stripMetadata: true,
      stripAttachments: true,
    };
    try {
      const report = applyRedactions(handle, plan);
      const nextBytes = savePdf(handle);
      const stableBytes = Uint8Array.from(nextBytes);
      setApplyReport(report);
      setStatus("Redactions applied. Reopening sanitized PDF...");
      await loadPdfBytes(stableBytes);
      setApplyReport(report);
      setDownloadBytes(stableBytes);
      setStatus("Sanitized PDF ready for download.");
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      setError(message);
      setStatus("Redaction failed.");
    }
  }

  function downloadSanitizedPdf() {
    if (!downloadBytes) return;
    const url = URL.createObjectURL(
      new Blob([Uint8Array.from(downloadBytes)], { type: "application/pdf" })
    );
    const anchor = window.document.createElement("a");
    anchor.href = url;
    anchor.download = "sanitized.pdf";
    anchor.style.display = "none";
    window.document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
  }

  const setRenderError = useCallback((pageIndex: number, message: string | null) => {
    setRenderErrors((current) => {
      const next = { ...current };
      if (message) next[pageIndex] = message;
      else delete next[pageIndex];
      return next;
    });
  }, []);

  return (
    <div className="app">
      <Toolbar
        onFileChange={onFileChange}
        status={status}
        error={error}
        zoom={zoom}
        onZoomChange={setZoom}
      />
      {pendingEncryptedBytes && (
        <form className="password-prompt" onSubmit={submitPasswordAttempt}>
          <label className="password-prompt-label">
            <span>Password</span>
            <input
              type="password"
              value={passwordInput}
              autoFocus
              onChange={(event) => setPasswordInput(event.target.value)}
              placeholder="User or owner password"
            />
          </label>
          <button type="submit" className="password-prompt-submit">
            Open
          </button>
          <button
            type="button"
            className="password-prompt-cancel"
            onClick={cancelPasswordPrompt}
          >
            Cancel
          </button>
        </form>
      )}
      <div className="main-layout">
        <Sidebar
          hasHandle={handle !== null}
          searchQuery={searchQuery}
          onSearchQueryChange={setSearchQuery}
          onSearch={runSearch}
          searchMatches={searchMatches.map((match) => ({
            text: match.text,
            pageIndex: match.pageIndex,
          }))}
          manualTargets={manualTargets}
          searchTargets={searchTargets}
          redactionMode={redactionMode}
          onRedactionModeChange={setRedactionMode}
          onApply={applyPlan}
          onClear={clearTargets}
          onDownload={downloadSanitizedPdf}
          applyReport={applyReport}
          downloadReady={downloadBytes !== null}
          error={error}
          pageTexts={pageTexts}
        />
        <div className="page-stage">
          {!pdfBytes ? (
            <div className="page-stage-empty">Open a PDF file to get started.</div>
          ) : (
            pageSizes.map((size, pageIndex) => (
              <PageView
                key={pageIndex}
                document={previewDocument}
                pageIndex={pageIndex}
                size={size}
                zoom={zoom}
                collapsed={collapsedPages.has(pageIndex)}
                onToggleCollapsed={() => togglePageCollapsed(pageIndex)}
                manualTargets={manualTargets.filter((target) => target.pageIndex === pageIndex)}
                searchTargets={searchTargets.filter((target) => target.pageIndex === pageIndex)}
                onCreateRectTarget={addManualTarget}
                onRenderError={setRenderError}
                renderError={renderErrors[pageIndex] ?? null}
              />
            ))
          )}
        </div>
      </div>
    </div>
  );
}

// --- Search match normalization ---

function normalizeSearchMatch(match: unknown): UiTextMatch | null {
  if (!match || typeof match !== "object") return null;
  const candidate = match as Record<string, unknown>;
  const pageIndex = readPageIndex(candidate);
  const quads = Array.isArray(candidate.quads)
    ? candidate.quads.filter(isQuadCandidate).map(readQuadPoints)
    : [];
  if (pageIndex === null || quads.length === 0 || typeof candidate.text !== "string") return null;
  return { text: candidate.text, pageIndex, quads };
}

function readPageIndex(candidate: Record<string, unknown>): number | null {
  const value = candidate.pageIndex ?? candidate.page_index;
  return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : null;
}

function isQuadPoints(value: unknown): value is [Point, Point, Point, Point] {
  return Array.isArray(value) && value.length === 4 && value.every(isPoint);
}

function isPoint(value: unknown): value is Point {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.x === "number" &&
    Number.isFinite(candidate.x) &&
    typeof candidate.y === "number" &&
    Number.isFinite(candidate.y)
  );
}

function isQuadCandidate(
  value: unknown
): value is [Point, Point, Point, Point] | { points: [Point, Point, Point, Point] } {
  return (
    isQuadPoints(value) ||
    (!!value &&
      typeof value === "object" &&
      "points" in value &&
      isQuadPoints((value as { points?: unknown }).points))
  );
}

function readQuadPoints(
  quad: [Point, Point, Point, Point] | { points: [Point, Point, Point, Point] }
): [Point, Point, Point, Point] {
  return Array.isArray(quad) ? quad : quad.points;
}
