import type { ApplyReport, PageSize, PageText, RedactionPlan, TextMatch } from "./types";

export type { ApplyReport, PageSize, PageText, RedactionPlan, RedactionTarget, TextItem, TextMatch } from "./types";
export type {
  FillColor,
  Point,
  QuadGroupTarget,
  QuadTarget,
  RectTarget,
} from "./types";

export type PdfHandle = {
  readonly __brand: "PdfHandle";
};

type WasmModule = {
  openPdf(input: Uint8Array): PdfHandle;
  getPageCount(handle: PdfHandle): number;
  getPageSize(handle: PdfHandle, pageIndex: number): PageSize;
  extractText(handle: PdfHandle, pageIndex: number): RawPageText;
  searchText(handle: PdfHandle, pageIndex: number, query: string): RawTextMatch[];
  applyRedactions(handle: PdfHandle, plan: RedactionPlan): ApplyReport;
  savePdf(handle: PdfHandle): Uint8Array;
};

type RawPoint = { x: number; y: number };
type RawQuad = { points: [RawPoint, RawPoint, RawPoint, RawPoint] } | [RawPoint, RawPoint, RawPoint, RawPoint];
type RawTextItem = {
  text: string;
  bbox: { x: number; y: number; width: number; height: number };
  quad?: RawQuad | null;
  char_start?: number;
  char_end?: number;
};
type RawPageText = {
  page_index: number;
  text: string;
  items: RawTextItem[];
};
type RawTextMatch = {
  text: string;
  page_index: number;
  quads: RawQuad[];
};

let wasmModule: WasmModule | null = null;

/**
 * Loads the generated WebAssembly module used by the browser-facing SDK.
 *
 * Call this once before opening PDFs.
 */
export async function initWasm(): Promise<void> {
  if (wasmModule) {
    return;
  }
  const module = await import("../vendor/pdf-wasm/pdf_wasm");
  wasmModule = module as unknown as WasmModule;
}

function requireWasm(): WasmModule {
  if (!wasmModule) {
    throw new Error("WASM module is not initialized. Call initWasm() first.");
  }
  return wasmModule;
}

/** Opens a PDF from raw bytes and returns an opaque handle. */
export function openPdf(input: Uint8Array): PdfHandle {
  return requireWasm().openPdf(input);
}

/** Returns the number of pages in the opened PDF. */
export function getPageCount(handle: PdfHandle): number {
  return requireWasm().getPageCount(handle);
}

/** Returns the normalized page size for a zero-based page index. */
export function getPageSize(handle: PdfHandle, pageIndex: number): PageSize {
  return requireWasm().getPageSize(handle, pageIndex);
}

/** Extracts page text and geometry for the supported subset. */
export function extractText(handle: PdfHandle, pageIndex: number): PageText {
  return normalizePageText(requireWasm().extractText(handle, pageIndex));
}

/** Searches page text in visual order and returns page-space match quads. */
export function searchText(handle: PdfHandle, pageIndex: number, query: string): TextMatch[] {
  return requireWasm()
    .searchText(handle, pageIndex, query)
    .map(normalizeTextMatch);
}

/** Applies a redaction plan to the opened handle in place. */
export function applyRedactions(handle: PdfHandle, plan: RedactionPlan): ApplyReport {
  return requireWasm().applyRedactions(handle, plan);
}

/** Saves the current document state as a new PDF byte array. */
export function savePdf(handle: PdfHandle): Uint8Array {
  return requireWasm().savePdf(handle);
}

function normalizePageText(raw: RawPageText): PageText {
  return {
    pageIndex: raw.page_index,
    text: raw.text,
    items: raw.items.map((item) => ({
      text: item.text,
      bbox: item.bbox,
      quad: item.quad ? normalizeQuad(item.quad) : undefined,
      charStart: item.char_start,
      charEnd: item.char_end,
    })),
  };
}

function normalizeTextMatch(raw: RawTextMatch): TextMatch {
  return {
    text: raw.text,
    pageIndex: raw.page_index,
    quads: raw.quads.map(normalizeQuad),
  };
}

function normalizeQuad(
  quad: RawQuad,
): [RawPoint, RawPoint, RawPoint, RawPoint] {
  return Array.isArray(quad) ? quad : quad.points;
}
