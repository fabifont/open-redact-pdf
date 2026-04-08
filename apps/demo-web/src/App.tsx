import {
  useEffect,
  useEffectEvent,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type PointerEvent,
} from "react";
import {
  applyRedactions,
  extractText,
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
  type RedactionPlan,
  type TextMatch,
} from "@openredact/ts-sdk";
import { GlobalWorkerOptions, getDocument, type PDFDocumentProxy } from "pdfjs-dist";

GlobalWorkerOptions.workerSrc = new URL(
  "pdfjs-dist/build/pdf.worker.min.mjs",
  import.meta.url,
).toString();

type PagePreviewProps = {
  bytes: Uint8Array;
  pageIndex: number;
  size: { width: number; height: number };
  manualTargets: RectTarget[];
  searchTargets: QuadGroupTarget[];
  onCreateRectTarget: (target: RectTarget) => void;
  onRenderError: (pageIndex: number, message: string | null) => void;
  renderError: string | null;
};

type DragState = {
  startX: number;
  startY: number;
  currentX: number;
  currentY: number;
};

export function App() {
  const [status, setStatus] = useState("Load a local PDF to start.");
  const [error, setError] = useState<string | null>(null);
  const [pdfBytes, setPdfBytes] = useState<Uint8Array | null>(null);
  const [handle, setHandle] = useState<PdfHandle | null>(null);
  const [pageSizes, setPageSizes] = useState<Array<{ width: number; height: number }>>([]);
  const [manualTargets, setManualTargets] = useState<RectTarget[]>([]);
  const [searchTargets, setSearchTargets] = useState<QuadGroupTarget[]>([]);
  const [searchMatches, setSearchMatches] = useState<TextMatch[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [downloadUrl, setDownloadUrl] = useState<string | null>(null);
  const [applyReport, setApplyReport] = useState<null | {
    pages_touched: number;
    text_glyphs_removed: number;
    path_paints_removed: number;
    image_draws_removed: number;
    annotations_removed: number;
    warnings: string[];
  }>(null);
  const [pageTexts, setPageTexts] = useState<Array<{ text: string; error: string | null }>>([]);
  const [renderErrors, setRenderErrors] = useState<Record<number, string>>({});

  useEffect(() => {
    return () => {
      if (downloadUrl) {
        URL.revokeObjectURL(downloadUrl);
      }
    };
  }, [downloadUrl]);

  async function loadPdfBytes(bytes: Uint8Array) {
    setError(null);
    setStatus("Initializing WebAssembly...");
    await initWasm();
    const nextHandle = openPdf(bytes);
    const count = getPageCount(nextHandle);
    const sizes = Array.from({ length: count }, (_, pageIndex) =>
      getPageSize(nextHandle, pageIndex),
    );
    setHandle(nextHandle);
    setPdfBytes(bytes);
    setPageSizes(sizes);
    setRenderErrors({});
    setManualTargets([]);
    setSearchTargets([]);
    setSearchMatches([]);
    setApplyReport(null);
    const texts = Array.from({ length: count }, (_, pageIndex) => {
      try {
        return { text: extractText(nextHandle, pageIndex).text, error: null };
      } catch (caught) {
        const message = caught instanceof Error ? caught.message : String(caught);
        return { text: "", error: message };
      }
    });
    const extractionFailures = texts.filter((entry) => entry.error !== null).length;
    setPageTexts(texts);
    if (downloadUrl) {
      URL.revokeObjectURL(downloadUrl);
      setDownloadUrl(null);
    }
    if (extractionFailures > 0) {
      setStatus(
        `Loaded ${count} page${count === 1 ? "" : "s"}; text extraction is unsupported on ${extractionFailures} page${extractionFailures === 1 ? "" : "s"}.`,
      );
    } else {
      setStatus(`Loaded ${count} page${count === 1 ? "" : "s"}.`);
    }
  }

  async function onFileChange(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    if (!file) {
      return;
    }
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      await loadPdfBytes(bytes);
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      setError(message);
      setStatus("Failed to load PDF.");
    }
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

  function runSearch() {
    if (!handle || !searchQuery.trim()) {
      return;
    }
    try {
      const matches: TextMatch[] = [];
      const failures: string[] = [];
      for (let pageIndex = 0; pageIndex < pageSizes.length; pageIndex += 1) {
        try {
          matches.push(...searchText(handle, pageIndex, searchQuery));
        } catch (caught) {
          const message = caught instanceof Error ? caught.message : String(caught);
          failures.push(`Page ${pageIndex + 1}: ${message}`);
        }
      }
      const targets = matches.map<QuadGroupTarget>((match) => ({
        kind: "quadGroup",
        pageIndex: match.page_index,
        quads: match.quads,
      }));
      setSearchMatches(matches);
      setSearchTargets(targets);
      setError(failures.length > 0 ? failures.join(" | ") : null);
      if (matches.length === 0 && failures.length > 0) {
        setStatus("Search is unavailable on one or more pages for this PDF subset.");
      } else {
        setStatus(`Found ${matches.length} text match${matches.length === 1 ? "" : "es"}.`);
      }
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      setError(message);
      setStatus("Search failed.");
    }
  }

  async function applyPlan() {
    if (!handle || !pdfBytes) {
      return;
    }
    const plan: RedactionPlan = {
      targets: [...manualTargets, ...searchTargets],
      removeIntersectingAnnotations: true,
      stripMetadata: true,
      stripAttachments: true,
    };
    try {
      const report = applyRedactions(handle, plan);
      const nextBytes = savePdf(handle);
      const stableBytes = Uint8Array.from(nextBytes);
      const url = URL.createObjectURL(
        new Blob([stableBytes], { type: "application/pdf" }),
      );
      if (downloadUrl) {
        URL.revokeObjectURL(downloadUrl);
      }
      setDownloadUrl(url);
      setApplyReport(report);
      setStatus("Redactions applied. Reopening sanitized PDF...");
      await loadPdfBytes(stableBytes);
      setApplyReport(report);
      setDownloadUrl(url);
      setStatus("Sanitized PDF ready for download.");
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      setError(message);
      setStatus("Redaction failed.");
    }
  }

  const targetCount = manualTargets.length + searchTargets.length;

  const setRenderError = useEffectEvent((pageIndex: number, message: string | null) => {
    setRenderErrors((current) => {
      const next = { ...current };
      if (message) {
        next[pageIndex] = message;
      } else {
        delete next[pageIndex];
      }
      return next;
    });
  });

  return (
    <div className="app-shell">
      <aside className="control-panel">
        <div className="panel-header">
          <p className="eyebrow">Browser-First PDF Redaction</p>
          <h1>Open Redact PDF Demo</h1>
          <p className="lede">
            This demo renders pages with PDF.js, but every redact/apply/save action runs through the Rust/WASM engine.
          </p>
        </div>

        <label className="file-picker">
          <span>Open local PDF</span>
          <input type="file" accept="application/pdf" onChange={onFileChange} />
        </label>

        <div className="panel-block">
          <h2>Search-Driven Targets</h2>
          <div className="search-row">
            <input
              value={searchQuery}
              onChange={(event) => setSearchQuery(event.target.value)}
              placeholder="Search text to redact"
            />
            <button type="button" onClick={runSearch} disabled={!handle}>
              Find
            </button>
          </div>
          <p className="muted">
            Matches compile into quad-group targets before redaction.
          </p>
        </div>

        <div className="panel-block">
          <h2>Plan</h2>
          <p>{targetCount} target(s) queued.</p>
          <p>{manualTargets.length} manual rectangles.</p>
          <p>{searchTargets.length} search-derived quad groups.</p>
          <div className="button-row">
            <button type="button" onClick={applyPlan} disabled={!handle || targetCount === 0}>
              Apply Redactions
            </button>
            <button type="button" className="ghost" onClick={clearTargets} disabled={targetCount === 0}>
              Clear
            </button>
          </div>
        </div>

        <div className="panel-block">
          <h2>Status</h2>
          <p>{status}</p>
          {error ? <p className="error">{error}</p> : null}
          {applyReport ? (
            <div className="report">
              <p>{applyReport.text_glyphs_removed} glyphs removed</p>
              <p>{applyReport.path_paints_removed} vector paints removed</p>
              <p>{applyReport.image_draws_removed} image draws removed</p>
              <p>{applyReport.annotations_removed} annotations removed</p>
              {applyReport.warnings.map((warning) => (
                <p key={warning} className="warning">
                  {warning}
                </p>
              ))}
            </div>
          ) : null}
          {downloadUrl ? (
            <a className="download-link" href={downloadUrl} download="sanitized.pdf">
              Download Sanitized PDF
            </a>
          ) : null}
        </div>

        <div className="panel-block">
          <h2>Extracted Text</h2>
          {pageTexts.length === 0 ? (
            <p className="muted">Load a PDF to inspect the retained text layer.</p>
          ) : (
            pageTexts.map((entry, index) => (
              <details key={index}>
                <summary>Page {index + 1}</summary>
                {entry.error ? (
                  <p className="warning">{entry.error}</p>
                ) : (
                  <pre>{entry.text || "[empty]"}</pre>
                )}
              </details>
            ))
          )}
        </div>

        {searchMatches.length > 0 ? (
          <div className="panel-block">
            <h2>Search Matches</h2>
            {searchMatches.map((match, index) => (
              <p key={`${match.page_index}-${index}`}>
                Page {match.page_index + 1}: {match.text}
              </p>
            ))}
          </div>
        ) : null}
      </aside>

      <main className="page-stage">
        {!pdfBytes ? (
          <div className="empty-state">
            <p>Load a fixture or your own text-based PDF. Drag on a page to add rectangle targets.</p>
          </div>
        ) : (
          pageSizes.map((size, pageIndex) => (
            <PagePreview
              key={pageIndex}
              bytes={pdfBytes}
              pageIndex={pageIndex}
              size={size}
              manualTargets={manualTargets.filter((target) => target.pageIndex === pageIndex)}
              searchTargets={searchTargets.filter((target) => target.pageIndex === pageIndex)}
              onCreateRectTarget={addManualTarget}
              onRenderError={setRenderError}
              renderError={renderErrors[pageIndex] ?? null}
            />
          ))
        )}
      </main>
    </div>
  );
}

function PagePreview({
  bytes,
  pageIndex,
  size,
  manualTargets,
  searchTargets,
  onCreateRectTarget,
  onRenderError,
  renderError,
}: PagePreviewProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const overlayRef = useRef<SVGSVGElement | null>(null);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [viewport, setViewport] = useState<{ width: number; height: number; scale: number } | null>(null);

  useEffect(() => {
    let cancelled = false;
    let documentRef: PDFDocumentProxy | null = null;
    async function renderPage() {
      const document = await getDocument({ data: bytes }).promise;
      documentRef = document;
      const page = await document.getPage(pageIndex + 1);
      const targetWidth = 720;
      const baseViewport = page.getViewport({ scale: 1 });
      const scale = Math.min(targetWidth / baseViewport.width, 1.4);
      const pageViewport = page.getViewport({ scale });
      if (cancelled || !canvasRef.current) {
        return;
      }
      const canvas = canvasRef.current;
      const context = canvas.getContext("2d");
      if (!context) {
        return;
      }
      canvas.width = pageViewport.width;
      canvas.height = pageViewport.height;
      await page
        .render({
          canvas,
          canvasContext: context,
          viewport: pageViewport,
        })
        .promise;
      if (!cancelled) {
        onRenderError(pageIndex, null);
        setViewport({
          width: pageViewport.width,
          height: pageViewport.height,
          scale: pageViewport.width / size.width,
        });
      }
    }
    renderPage().catch((caught) => {
      if (!cancelled) {
        const message = caught instanceof Error ? caught.message : String(caught);
        onRenderError(pageIndex, message);
      }
    });
    return () => {
      cancelled = true;
      void documentRef?.destroy();
    };
  }, [bytes, onRenderError, pageIndex, size.width]);

  const draftRect = useMemo(() => {
    if (!drag || !viewport) {
      return null;
    }
    const x = Math.min(drag.startX, drag.currentX);
    const y = Math.min(drag.startY, drag.currentY);
    return {
      x,
      y,
      width: Math.abs(drag.currentX - drag.startX),
      height: Math.abs(drag.currentY - drag.startY),
    };
  }, [drag, viewport]);

  function beginDrag(event: PointerEvent<SVGSVGElement>) {
    if (!viewport || !overlayRef.current) {
      return;
    }
    const { x, y } = clientToOverlay(event, overlayRef.current);
    setDrag({ startX: x, startY: y, currentX: x, currentY: y });
  }

  function updateDrag(event: PointerEvent<SVGSVGElement>) {
    if (!drag || !overlayRef.current) {
      return;
    }
    const { x, y } = clientToOverlay(event, overlayRef.current);
    setDrag({ ...drag, currentX: x, currentY: y });
  }

  function finishDrag() {
    if (!drag || !viewport) {
      setDrag(null);
      return;
    }
    const width = Math.abs(drag.currentX - drag.startX);
    const height = Math.abs(drag.currentY - drag.startY);
    if (width > 8 && height > 8) {
      const left = Math.min(drag.startX, drag.currentX);
      const top = Math.min(drag.startY, drag.currentY);
      onCreateRectTarget({
        kind: "rect",
        pageIndex,
        x: left / viewport.scale,
        y: size.height - (top + height) / viewport.scale,
        width: width / viewport.scale,
        height: height / viewport.scale,
      });
    }
    setDrag(null);
  }

  return (
    <section className="page-card">
      <header>
        <h3>Page {pageIndex + 1}</h3>
        <p>
          {Math.round(size.width)} × {Math.round(size.height)} pt
        </p>
      </header>
      <div className="page-canvas-wrap">
        <canvas ref={canvasRef} className="page-canvas" />
        {renderError ? <div className="page-render-error">{renderError}</div> : null}
        {viewport ? (
          <svg
            ref={overlayRef}
            className="page-overlay"
            viewBox={`0 0 ${viewport.width} ${viewport.height}`}
            onPointerDown={beginDrag}
            onPointerMove={updateDrag}
            onPointerUp={finishDrag}
            onPointerLeave={finishDrag}
          >
            {manualTargets.map((target, index) => (
              <rect
                key={`manual-${pageIndex}-${index}`}
                className="manual-target"
                x={target.x * viewport.scale}
                y={(size.height - target.y - target.height) * viewport.scale}
                width={target.width * viewport.scale}
                height={target.height * viewport.scale}
              />
            ))}
            {searchTargets.flatMap((target, targetIndex) =>
              target.quads.map((quad: QuadGroupTarget["quads"][number], quadIndex: number) => (
                <polygon
                  key={`search-${targetIndex}-${quadIndex}`}
                  className="search-target"
                  points={quad
                    .map((point: Point) => toSvgPoint(point, size.height, viewport.scale))
                    .join(" ")}
                />
              )),
            )}
            {draftRect ? (
              <rect
                className="draft-target"
                x={draftRect.x}
                y={draftRect.y}
                width={draftRect.width}
                height={draftRect.height}
              />
            ) : null}
          </svg>
        ) : null}
      </div>
    </section>
  );
}

function clientToOverlay(
  event: PointerEvent<SVGSVGElement>,
  element: SVGSVGElement,
) {
  const bounds = element.getBoundingClientRect();
  return {
    x: event.clientX - bounds.left,
    y: event.clientY - bounds.top,
  };
}

function toSvgPoint(point: Point, pageHeight: number, scale: number): string {
  return `${point.x * scale},${(pageHeight - point.y) * scale}`;
}
