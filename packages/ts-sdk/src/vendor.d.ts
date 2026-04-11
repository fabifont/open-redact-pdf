declare module "../vendor/pdf-wasm/pdf_wasm" {
  type WasmPoint = { x: number; y: number };
  type WasmQuad = { points: [WasmPoint, WasmPoint, WasmPoint, WasmPoint] };

  export function openPdf(input: Uint8Array): unknown;
  export function getPageCount(handle: unknown): number;
  export function getPageSize(
    handle: unknown,
    pageIndex: number
  ): { width: number; height: number };
  export function extractText(
    handle: unknown,
    pageIndex: number
  ): {
    page_index: number;
    text: string;
    items: Array<{
      text: string;
      bbox: { x: number; y: number; width: number; height: number };
      quad?: WasmQuad | null;
      char_start?: number;
      char_end?: number;
    }>;
  };
  export function searchText(
    handle: unknown,
    pageIndex: number,
    query: string
  ): Array<{
    text: string;
    page_index: number;
    quads: WasmQuad[];
  }>;
  export function applyRedactions(handle: unknown, plan: unknown): unknown;
  export function savePdf(handle: unknown): Uint8Array;
}
