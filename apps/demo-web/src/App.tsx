import { useCallback, useEffect, useState, type ChangeEvent } from "react";
import {
  applyRedactions,
  extractText,
  freePdf,
  getPageCount,
  getPageSize,
  initWasm,
  openPdf,
  savePdf,
  searchText,
  type PdfHandle,
  type Point,
  type QuadGroupTarget,
  type RectTarget,
  type RedactionMode,
  type RedactionPlan,
  type ApplyReport,
} from "@open-redact-pdf/sdk";
import {
  GlobalWorkerOptions,
  getDocument,
  type PDFDocumentProxy,
} from "pdfjs-dist";
import { Toolbar } from "./components/Toolbar";
import { Sidebar } from "./components/Sidebar";
import { PageView } from "./components/PageView";

GlobalWorkerOptions.workerSrc = new URL(
  "pdfjs-dist/build/pdf.worker.min.mjs",
  import.meta.url,
).toString();

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
  const [pageSizes, setPageSizes] = useState<
    Array<{ width: number; height: number }>
  >([]);
  const [manualTargets, setManualTargets] = useState<RectTarget[]>([]);
  const [searchTargets, setSearchTargets] = useState<QuadGroupTarget[]>([]);
  const [searchMatches, setSearchMatches] = useState<UiTextMatch[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [downloadBytes, setDownloadBytes] = useState<Uint8Array | null>(null);
  const [applyReport, setApplyReport] = useState<ApplyReport | null>(null);
  const [redactionMode, setRedactionMode] = useState<RedactionMode>("redact");
  const [pageTexts, setPageTexts] = useState<
    Array<{ text: string; error: string | null }>
  >([]);
  const [renderErrors, setRenderErrors] = useState<Record<number, string>>({});
  const [previewDocument, setPreviewDocument] =
    useState<PDFDocumentProxy | null>(null);

  // PDF.js document lifecycle
  useEffect(() => {
    if (!pdfBytes) {
      setPreviewDocument(null);
      return;
    }
    let cancelled = false;
    let docRef: PDFDocumentProxy | null = null;
    setRenderErrors({});
    getDocument({ data: Uint8Array.from(pdfBytes) })
      .promise.then((doc) => {
        docRef = doc;
        if (!cancelled) setPreviewDocument(doc);
      })
      .catch((caught) => {
        const msg = caught instanceof Error ? caught.message : String(caught);
        if (!cancelled) {
          setError(msg);
          setStatus("PDF.js preview failed.");
          setPreviewDocument(null);
        }
      });
    return () => {
      cancelled = true;
      setPreviewDocument((cur) => (cur === docRef ? null : cur));
      void docRef?.destroy();
    };
  }, [pdfBytes]);

  async function loadPdfBytes(bytes: Uint8Array) {
    setError(null);
    setStatus("Initializing WebAssembly...");
    await initWasm();
    if (handle) {
      freePdf(handle);
      setHandle(null);
    }
    const nextHandle = openPdf(Uint8Array.from(bytes));
    const count = getPageCount(nextHandle);
    const sizes = Array.from({ length: count }, (_, i) =>
      getPageSize(nextHandle, i),
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
    const texts = Array.from({ length: count }, (_, i) => {
      try {
        return { text: extractText(nextHandle, i).text, error: null };
      } catch (caught) {
        const msg = caught instanceof Error ? caught.message : String(caught);
        return { text: "", error: msg };
      }
    });
    setPageTexts(texts);
    const failures = texts.filter((e) => e.error !== null).length;
    if (failures > 0) {
      setStatus(
        `Loaded ${count} page${count === 1 ? "" : "s"}; text extraction unsupported on ${failures} page${failures === 1 ? "" : "s"}.`,
      );
    } else {
      setStatus(`Loaded ${count} page${count === 1 ? "" : "s"}.`);
    }
  }

  async function onFileChange(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    if (!file) return;
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      await loadPdfBytes(bytes);
    } catch (caught) {
      const msg = caught instanceof Error ? caught.message : String(caught);
      setError(msg);
      setStatus("Failed to load PDF.");
    }
  }

  function addManualTarget(target: RectTarget) {
    setManualTargets((cur) => [...cur, target]);
  }

  function clearTargets() {
    setManualTargets([]);
    setSearchTargets([]);
    setSearchMatches([]);
    setApplyReport(null);
  }

  function runSearch() {
    if (!handle || !searchQuery.trim()) return;
    try {
      const matches: UiTextMatch[] = [];
      const failures: string[] = [];
      for (let i = 0; i < pageSizes.length; i++) {
        try {
          for (const match of searchText(handle, i, searchQuery)) {
            const normalized = normalizeSearchMatch(match);
            if (normalized) {
              matches.push(normalized);
            } else {
              failures.push(
                `Page ${i + 1}: search result geometry was invalid`,
              );
            }
          }
        } catch (caught) {
          const msg =
            caught instanceof Error ? caught.message : String(caught);
          failures.push(`Page ${i + 1}: ${msg}`);
        }
      }
      const targets = matches.map<QuadGroupTarget>((m) => ({
        kind: "quadGroup",
        pageIndex: m.pageIndex,
        quads: m.quads,
      }));
      setSearchMatches(matches);
      setSearchTargets(targets);
      setError(failures.length > 0 ? failures.join(" | ") : null);
      if (matches.length === 0 && failures.length > 0) {
        setStatus("Search unavailable on one or more pages.");
      } else {
        setStatus(
          `Found ${matches.length} match${matches.length === 1 ? "" : "es"}.`,
        );
      }
    } catch (caught) {
      const msg = caught instanceof Error ? caught.message : String(caught);
      setError(msg);
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
      const msg = caught instanceof Error ? caught.message : String(caught);
      setError(msg);
      setStatus("Redaction failed.");
    }
  }

  function downloadSanitizedPdf() {
    if (!downloadBytes) return;
    const url = URL.createObjectURL(
      new Blob([Uint8Array.from(downloadBytes)], { type: "application/pdf" }),
    );
    const a = window.document.createElement("a");
    a.href = url;
    a.download = "sanitized.pdf";
    a.style.display = "none";
    window.document.body.appendChild(a);
    a.click();
    a.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
  }

  const setRenderError = useCallback(
    (pageIndex: number, message: string | null) => {
      setRenderErrors((cur) => {
        const next = { ...cur };
        if (message) next[pageIndex] = message;
        else delete next[pageIndex];
        return next;
      });
    },
    [],
  );

  return (
    <div className="app">
      <Toolbar onFileChange={onFileChange} status={status} error={error} />
      <div className="main-layout">
        <Sidebar
          hasHandle={handle !== null}
          searchQuery={searchQuery}
          onSearchQueryChange={setSearchQuery}
          onSearch={runSearch}
          searchMatches={searchMatches.map((m) => ({
            text: m.text,
            pageIndex: m.pageIndex,
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
          pageTexts={pageTexts}
        />
        <div className="page-stage">
          {!pdfBytes ? (
            <div className="page-stage-empty">
              Open a PDF file to get started.
            </div>
          ) : (
            pageSizes.map((size, i) => (
              <PageView
                key={i}
                document={previewDocument}
                pageIndex={i}
                size={size}
                manualTargets={manualTargets.filter(
                  (t) => t.pageIndex === i,
                )}
                searchTargets={searchTargets.filter(
                  (t) => t.pageIndex === i,
                )}
                onCreateRectTarget={addManualTarget}
                onRenderError={setRenderError}
                renderError={renderErrors[i] ?? null}
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
  const c = match as Record<string, unknown>;
  const pageIndex = readPageIndex(c);
  const quads = Array.isArray(c.quads)
    ? c.quads.filter(isQuadCandidate).map(readQuadPoints)
    : [];
  if (pageIndex === null || quads.length === 0 || typeof c.text !== "string")
    return null;
  return { text: c.text, pageIndex, quads };
}

function readPageIndex(c: Record<string, unknown>): number | null {
  const v = c.pageIndex ?? c.page_index;
  return typeof v === "number" && Number.isInteger(v) && v >= 0 ? v : null;
}

function isQuadPoints(q: unknown): q is [Point, Point, Point, Point] {
  return Array.isArray(q) && q.length === 4 && q.every(isPoint);
}

function isPoint(p: unknown): p is Point {
  if (!p || typeof p !== "object") return false;
  const c = p as Record<string, unknown>;
  return (
    typeof c.x === "number" &&
    Number.isFinite(c.x) &&
    typeof c.y === "number" &&
    Number.isFinite(c.y)
  );
}

function isQuadCandidate(
  q: unknown,
): q is
  | [Point, Point, Point, Point]
  | { points: [Point, Point, Point, Point] } {
  return (
    isQuadPoints(q) ||
    (!!q &&
      typeof q === "object" &&
      "points" in q &&
      isQuadPoints((q as { points?: unknown }).points))
  );
}

function readQuadPoints(
  q:
    | [Point, Point, Point, Point]
    | { points: [Point, Point, Point, Point] },
): [Point, Point, Point, Point] {
  return Array.isArray(q) ? q : q.points;
}
