import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent,
} from "react";
import type { PDFDocumentProxy } from "pdfjs-dist";
import type { Point, QuadGroupTarget, RectTarget } from "@open-redact-pdf/sdk";

type PageViewProps = {
  document: PDFDocumentProxy | null;
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

type Viewport = { width: number; height: number; scale: number };

export function PageView({
  document,
  pageIndex,
  size,
  manualTargets,
  searchTargets,
  onCreateRectTarget,
  onRenderError,
  renderError,
}: PageViewProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const overlayRef = useRef<SVGSVGElement | null>(null);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [viewport, setViewport] = useState<Viewport | null>(null);

  useEffect(() => {
    if (!document) {
      setViewport(null);
      return;
    }
    const doc = document;
    let cancelled = false;

    async function renderPage() {
      const page = await doc.getPage(pageIndex + 1);
      const targetWidth = 720;
      const baseViewport = page.getViewport({ scale: 1 });
      const scale = Math.min(targetWidth / baseViewport.width, 1.4);
      const pageViewport = page.getViewport({ scale });
      if (cancelled || !canvasRef.current) return;
      const canvas = canvasRef.current;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      canvas.width = pageViewport.width;
      canvas.height = pageViewport.height;
      await page
        .render({ canvas, canvasContext: ctx, viewport: pageViewport })
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
        const message =
          caught instanceof Error ? caught.message : String(caught);
        onRenderError(pageIndex, message);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [document, onRenderError, pageIndex, size.width]);

  const draftRect = useMemo(() => {
    if (!drag || !viewport) return null;
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
    if (!viewport || !overlayRef.current) return;
    const pt = clientToOverlay(event, overlayRef.current);
    setDrag({ startX: pt.x, startY: pt.y, currentX: pt.x, currentY: pt.y });
  }

  function updateDrag(event: PointerEvent<SVGSVGElement>) {
    if (!drag || !overlayRef.current) return;
    const pt = clientToOverlay(event, overlayRef.current);
    setDrag({ ...drag, currentX: pt.x, currentY: pt.y });
  }

  function finishDrag() {
    if (!drag || !viewport) {
      setDrag(null);
      return;
    }
    const w = Math.abs(drag.currentX - drag.startX);
    const h = Math.abs(drag.currentY - drag.startY);
    if (w > 8 && h > 8) {
      const left = Math.min(drag.startX, drag.currentX);
      const top = Math.min(drag.startY, drag.currentY);
      onCreateRectTarget({
        kind: "rect",
        pageIndex,
        x: left / viewport.scale,
        y: size.height - (top + h) / viewport.scale,
        width: w / viewport.scale,
        height: h / viewport.scale,
      });
    }
    setDrag(null);
  }

  return (
    <div className="page-card">
      <div className="page-card-header">
        <span>Page {pageIndex + 1}</span>
        <span>
          {Math.round(size.width)} x {Math.round(size.height)} pt
        </span>
      </div>
      <div className="page-canvas-wrap">
        <canvas ref={canvasRef} />
        {renderError ? (
          <div className="page-render-error">{renderError}</div>
        ) : null}
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
            {manualTargets.map((target, i) => (
              <rect
                key={`r-${pageIndex}-${i}`}
                className="target-rect"
                x={target.x * viewport.scale}
                y={(size.height - target.y - target.height) * viewport.scale}
                width={target.width * viewport.scale}
                height={target.height * viewport.scale}
              />
            ))}
            {searchTargets.flatMap((t) => t.quads).map((quad, i) => (
              <polygon
                key={`q-${pageIndex}-${i}`}
                className="target-quad"
                points={quad
                  .map((p: Point) => toSvgPoint(p, size.height, viewport.scale))
                  .join(" ")}
              />
            ))}
            {draftRect ? (
              <rect
                className="target-draft"
                x={draftRect.x}
                y={draftRect.y}
                width={draftRect.width}
                height={draftRect.height}
              />
            ) : null}
          </svg>
        ) : null}
      </div>
    </div>
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

function toSvgPoint(
  point: Point,
  pageHeight: number,
  scale: number,
): string {
  return `${point.x * scale},${(pageHeight - point.y) * scale}`;
}
