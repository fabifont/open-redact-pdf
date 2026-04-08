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
  extractText(handle: PdfHandle, pageIndex: number): PageText;
  searchText(handle: PdfHandle, pageIndex: number, query: string): TextMatch[];
  applyRedactions(handle: PdfHandle, plan: RedactionPlan): ApplyReport;
  savePdf(handle: PdfHandle): Uint8Array;
};

let wasmModule: WasmModule | null = null;

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

export function openPdf(input: Uint8Array): PdfHandle {
  return requireWasm().openPdf(input);
}

export function getPageCount(handle: PdfHandle): number {
  return requireWasm().getPageCount(handle);
}

export function getPageSize(handle: PdfHandle, pageIndex: number): PageSize {
  return requireWasm().getPageSize(handle, pageIndex);
}

export function extractText(handle: PdfHandle, pageIndex: number): PageText {
  return requireWasm().extractText(handle, pageIndex);
}

export function searchText(handle: PdfHandle, pageIndex: number, query: string): TextMatch[] {
  return requireWasm().searchText(handle, pageIndex, query);
}

export function applyRedactions(handle: PdfHandle, plan: RedactionPlan): ApplyReport {
  return requireWasm().applyRedactions(handle, plan);
}

export function savePdf(handle: PdfHandle): Uint8Array {
  return requireWasm().savePdf(handle);
}
