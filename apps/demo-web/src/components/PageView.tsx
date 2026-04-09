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
  zoom: number;
  collapsed: boolean;
  onToggleCollapsed: () => void;
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
  zoom,
  collapsed,
  onToggleCollapsed,
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
  const [renderedWidth, setRenderedWidth] = useState<number | null>(null);

  useEffect(() => {
    if (!document || collapsed) {
      setViewport(null);
      return;
    }
    const pdfDocument = document;
    let cancelled = false;

    async function renderPage() {
      const page = await pdfDocument.getPage(pageIndex + 1);
      const baseViewport = page.getViewport({ scale: 1 });
      const baseScale = Math.min(720 / baseViewport.width, 1.4);
      const renderScale = baseScale * zoom;
      const pageViewport = page.getViewport({ scale: renderScale });
      if (cancelled || !canvasRef.current) return;
      const canvas = canvasRef.current;
      const context = canvas.getContext("2d");
      if (!context) return;
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
        setRenderedWidth(pageViewport.width);
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
  }, [document, onRenderError, pageIndex, size.width, zoom, collapsed]);

  const draftRect = useMemo(() => {
    if (!drag || !viewport) return null;
    const left = Math.min(drag.startX, drag.currentX);
    const top = Math.min(drag.startY, drag.currentY);
    return {
      x: left,
      y: top,
      width: Math.abs(drag.currentX - drag.startX),
      height: Math.abs(drag.currentY - drag.startY),
    };
  }, [drag, viewport]);

  function beginDrag(event: PointerEvent<SVGSVGElement>) {
    if (!viewport || !overlayRef.current) return;
    const point = clientToOverlay(event, overlayRef.current);
    setDrag({
      startX: point.x,
      startY: point.y,
      currentX: point.x,
      currentY: point.y,
    });
  }

  function updateDrag(event: PointerEvent<SVGSVGElement>) {
    if (!drag || !overlayRef.current) return;
    const point = clientToOverlay(event, overlayRef.current);
    setDrag({ ...drag, currentX: point.x, currentY: point.y });
  }

  function finishDrag() {
    if (!drag || !viewport) {
      setDrag(null);
      return;
    }
    const dragWidth = Math.abs(drag.currentX - drag.startX);
    const dragHeight = Math.abs(drag.currentY - drag.startY);
    if (dragWidth > 8 && dragHeight > 8) {
      const left = Math.min(drag.startX, drag.currentX);
      const top = Math.min(drag.startY, drag.currentY);
      onCreateRectTarget({
        kind: "rect",
        pageIndex,
        x: left / viewport.scale,
        y: size.height - (top + dragHeight) / viewport.scale,
        width: dragWidth / viewport.scale,
        height: dragHeight / viewport.scale,
      });
    }
    setDrag(null);
  }

  const targetCount = manualTargets.length + searchTargets.reduce((sum, target) => sum + target.quads.length, 0);

  return (
    <div
      className={`page-card${collapsed ? " page-card-collapsed" : ""}`}
      style={collapsed && renderedWidth ? { minWidth: renderedWidth } : undefined}
    >
      <div
        className="page-card-header"
        onClick={onToggleCollapsed}
        role="button"
        tabIndex={0}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === " ") onToggleCollapsed();
        }}
      >
        <span>
          <span className="page-collapse-icon">
            {collapsed ? "\u25b6" : "\u25bc"}
          </span>
          Page {pageIndex + 1}
          {targetCount > 0 ? (
            <span className="page-target-badge">{targetCount}</span>
          ) : null}
        </span>
        <span>
          {Math.round(size.width)} x {Math.round(size.height)} pt
        </span>
      </div>
      {!collapsed ? (
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
              {manualTargets.map((target, index) => (
                <rect
                  key={`rect-${pageIndex}-${index}`}
                  className="target-rect"
                  x={target.x * viewport.scale}
                  y={
                    (size.height - target.y - target.height) * viewport.scale
                  }
                  width={target.width * viewport.scale}
                  height={target.height * viewport.scale}
                />
              ))}
              {searchTargets
                .flatMap((target) => target.quads)
                .map((quad, index) => (
                  <polygon
                    key={`quad-${pageIndex}-${index}`}
                    className="target-quad"
                    points={quad
                      .map((point: Point) =>
                        toSvgPoint(point, size.height, viewport.scale),
                      )
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
      ) : null}
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
