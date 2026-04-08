import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useEffect, useMemo, useRef, useState, } from "react";
import { applyRedactions, extractText, getPageCount, getPageSize, initWasm, openPdf, savePdf, searchText, } from "@openredact/ts-sdk";
import { GlobalWorkerOptions, getDocument } from "pdfjs-dist";
GlobalWorkerOptions.workerSrc = new URL("pdfjs-dist/build/pdf.worker.min.mjs", import.meta.url).toString();
export function App() {
    const [status, setStatus] = useState("Load a local PDF to start.");
    const [error, setError] = useState(null);
    const [pdfBytes, setPdfBytes] = useState(null);
    const [handle, setHandle] = useState(null);
    const [pageSizes, setPageSizes] = useState([]);
    const [manualTargets, setManualTargets] = useState([]);
    const [searchTargets, setSearchTargets] = useState([]);
    const [searchMatches, setSearchMatches] = useState([]);
    const [searchQuery, setSearchQuery] = useState("");
    const [downloadUrl, setDownloadUrl] = useState(null);
    const [applyReport, setApplyReport] = useState(null);
    const [pageTexts, setPageTexts] = useState([]);
    useEffect(() => {
        return () => {
            if (downloadUrl) {
                URL.revokeObjectURL(downloadUrl);
            }
        };
    }, [downloadUrl]);
    async function loadPdfBytes(bytes) {
        setError(null);
        setStatus("Initializing WebAssembly...");
        await initWasm();
        const nextHandle = openPdf(bytes);
        const count = getPageCount(nextHandle);
        const sizes = Array.from({ length: count }, (_, pageIndex) => getPageSize(nextHandle, pageIndex));
        const texts = Array.from({ length: count }, (_, pageIndex) => extractText(nextHandle, pageIndex).text);
        setHandle(nextHandle);
        setPdfBytes(bytes);
        setPageSizes(sizes);
        setPageTexts(texts);
        setManualTargets([]);
        setSearchTargets([]);
        setSearchMatches([]);
        setApplyReport(null);
        if (downloadUrl) {
            URL.revokeObjectURL(downloadUrl);
            setDownloadUrl(null);
        }
        setStatus(`Loaded ${count} page${count === 1 ? "" : "s"}.`);
    }
    async function onFileChange(event) {
        const file = event.target.files?.[0];
        if (!file) {
            return;
        }
        try {
            const bytes = new Uint8Array(await file.arrayBuffer());
            await loadPdfBytes(bytes);
        }
        catch (caught) {
            const message = caught instanceof Error ? caught.message : String(caught);
            setError(message);
            setStatus("Failed to load PDF.");
        }
    }
    function addManualTarget(target) {
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
            const matches = Array.from({ length: pageSizes.length }, (_, pageIndex) => searchText(handle, pageIndex, searchQuery)).flat();
            const targets = matches.map((match) => ({
                kind: "quadGroup",
                pageIndex: match.page_index,
                quads: match.quads,
            }));
            setSearchMatches(matches);
            setSearchTargets(targets);
            setStatus(`Found ${matches.length} text match${matches.length === 1 ? "" : "es"}.`);
        }
        catch (caught) {
            const message = caught instanceof Error ? caught.message : String(caught);
            setError(message);
            setStatus("Search failed.");
        }
    }
    async function applyPlan() {
        if (!handle || !pdfBytes) {
            return;
        }
        const plan = {
            targets: [...manualTargets, ...searchTargets],
            removeIntersectingAnnotations: true,
            stripMetadata: true,
            stripAttachments: true,
        };
        try {
            const report = applyRedactions(handle, plan);
            const nextBytes = savePdf(handle);
            const stableBytes = Uint8Array.from(nextBytes);
            const url = URL.createObjectURL(new Blob([stableBytes], { type: "application/pdf" }));
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
        }
        catch (caught) {
            const message = caught instanceof Error ? caught.message : String(caught);
            setError(message);
            setStatus("Redaction failed.");
        }
    }
    const targetCount = manualTargets.length + searchTargets.length;
    return (_jsxs("div", { className: "app-shell", children: [_jsxs("aside", { className: "control-panel", children: [_jsxs("div", { className: "panel-header", children: [_jsx("p", { className: "eyebrow", children: "Browser-First PDF Redaction" }), _jsx("h1", { children: "Open Redact PDF Demo" }), _jsx("p", { className: "lede", children: "This demo renders pages with PDF.js, but every redact/apply/save action runs through the Rust/WASM engine." })] }), _jsxs("label", { className: "file-picker", children: [_jsx("span", { children: "Open local PDF" }), _jsx("input", { type: "file", accept: "application/pdf", onChange: onFileChange })] }), _jsxs("div", { className: "panel-block", children: [_jsx("h2", { children: "Search-Driven Targets" }), _jsxs("div", { className: "search-row", children: [_jsx("input", { value: searchQuery, onChange: (event) => setSearchQuery(event.target.value), placeholder: "Search text to redact" }), _jsx("button", { type: "button", onClick: runSearch, disabled: !handle, children: "Find" })] }), _jsx("p", { className: "muted", children: "Matches compile into quad-group targets before redaction." })] }), _jsxs("div", { className: "panel-block", children: [_jsx("h2", { children: "Plan" }), _jsxs("p", { children: [targetCount, " target(s) queued."] }), _jsxs("p", { children: [manualTargets.length, " manual rectangles."] }), _jsxs("p", { children: [searchTargets.length, " search-derived quad groups."] }), _jsxs("div", { className: "button-row", children: [_jsx("button", { type: "button", onClick: applyPlan, disabled: !handle || targetCount === 0, children: "Apply Redactions" }), _jsx("button", { type: "button", className: "ghost", onClick: clearTargets, disabled: targetCount === 0, children: "Clear" })] })] }), _jsxs("div", { className: "panel-block", children: [_jsx("h2", { children: "Status" }), _jsx("p", { children: status }), error ? _jsx("p", { className: "error", children: error }) : null, applyReport ? (_jsxs("div", { className: "report", children: [_jsxs("p", { children: [applyReport.text_glyphs_removed, " glyphs removed"] }), _jsxs("p", { children: [applyReport.path_paints_removed, " vector paints removed"] }), _jsxs("p", { children: [applyReport.image_draws_removed, " image draws removed"] }), _jsxs("p", { children: [applyReport.annotations_removed, " annotations removed"] }), applyReport.warnings.map((warning) => (_jsx("p", { className: "warning", children: warning }, warning)))] })) : null, downloadUrl ? (_jsx("a", { className: "download-link", href: downloadUrl, download: "sanitized.pdf", children: "Download Sanitized PDF" })) : null] }), _jsxs("div", { className: "panel-block", children: [_jsx("h2", { children: "Extracted Text" }), pageTexts.length === 0 ? (_jsx("p", { className: "muted", children: "Load a PDF to inspect the retained text layer." })) : (pageTexts.map((text, index) => (_jsxs("details", { children: [_jsxs("summary", { children: ["Page ", index + 1] }), _jsx("pre", { children: text || "[empty]" })] }, index))))] }), searchMatches.length > 0 ? (_jsxs("div", { className: "panel-block", children: [_jsx("h2", { children: "Search Matches" }), searchMatches.map((match, index) => (_jsxs("p", { children: ["Page ", match.page_index + 1, ": ", match.text] }, `${match.page_index}-${index}`)))] })) : null] }), _jsx("main", { className: "page-stage", children: !pdfBytes ? (_jsx("div", { className: "empty-state", children: _jsx("p", { children: "Load a fixture or your own text-based PDF. Drag on a page to add rectangle targets." }) })) : (pageSizes.map((size, pageIndex) => (_jsx(PagePreview, { bytes: pdfBytes, pageIndex: pageIndex, size: size, manualTargets: manualTargets.filter((target) => target.pageIndex === pageIndex), searchTargets: searchTargets.filter((target) => target.pageIndex === pageIndex), onCreateRectTarget: addManualTarget }, pageIndex)))) })] }));
}
function PagePreview({ bytes, pageIndex, size, manualTargets, searchTargets, onCreateRectTarget, }) {
    const canvasRef = useRef(null);
    const overlayRef = useRef(null);
    const [drag, setDrag] = useState(null);
    const [viewport, setViewport] = useState(null);
    useEffect(() => {
        let cancelled = false;
        let documentRef = null;
        async function renderPage() {
            const document = await getDocument({ data: bytes }).promise;
            documentRef = document;
            const page = await document.getPage(pageIndex + 1);
            const targetWidth = 720;
            const scale = Math.min(targetWidth / page.view[2], 1.4);
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
                setViewport({
                    width: pageViewport.width,
                    height: pageViewport.height,
                    scale: pageViewport.width / size.width,
                });
            }
        }
        renderPage().catch(console.error);
        return () => {
            cancelled = true;
            void documentRef?.destroy();
        };
    }, [bytes, pageIndex, size.width]);
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
    function beginDrag(event) {
        if (!viewport || !overlayRef.current) {
            return;
        }
        const { x, y } = clientToOverlay(event, overlayRef.current);
        setDrag({ startX: x, startY: y, currentX: x, currentY: y });
    }
    function updateDrag(event) {
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
    return (_jsxs("section", { className: "page-card", children: [_jsxs("header", { children: [_jsxs("h3", { children: ["Page ", pageIndex + 1] }), _jsxs("p", { children: [Math.round(size.width), " \u00D7 ", Math.round(size.height), " pt"] })] }), _jsxs("div", { className: "page-canvas-wrap", children: [_jsx("canvas", { ref: canvasRef, className: "page-canvas" }), viewport ? (_jsxs("svg", { ref: overlayRef, className: "page-overlay", viewBox: `0 0 ${viewport.width} ${viewport.height}`, onPointerDown: beginDrag, onPointerMove: updateDrag, onPointerUp: finishDrag, onPointerLeave: finishDrag, children: [manualTargets.map((target, index) => (_jsx("rect", { className: "manual-target", x: target.x * viewport.scale, y: (size.height - target.y - target.height) * viewport.scale, width: target.width * viewport.scale, height: target.height * viewport.scale }, `manual-${pageIndex}-${index}`))), searchTargets.flatMap((target, targetIndex) => target.quads.map((quad, quadIndex) => (_jsx("polygon", { className: "search-target", points: quad
                                    .map((point) => toSvgPoint(point, size.height, viewport.scale))
                                    .join(" ") }, `search-${targetIndex}-${quadIndex}`)))), draftRect ? (_jsx("rect", { className: "draft-target", x: draftRect.x, y: draftRect.y, width: draftRect.width, height: draftRect.height })) : null] })) : null] })] }));
}
function clientToOverlay(event, element) {
    const bounds = element.getBoundingClientRect();
    return {
        x: event.clientX - bounds.left,
        y: event.clientY - bounds.top,
    };
}
function toSvgPoint(point, pageHeight, scale) {
    return `${point.x * scale},${(pageHeight - point.y) * scale}`;
}
