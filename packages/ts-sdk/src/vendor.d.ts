declare module "../vendor/pdf-wasm/pdf_wasm" {
  export function openPdf(input: Uint8Array): unknown;
  export function getPageCount(handle: unknown): number;
  export function getPageSize(
    handle: unknown,
    pageIndex: number,
  ): { width: number; height: number };
  export function extractText(
    handle: unknown,
    pageIndex: number,
  ): { page_index: number; text: string; items: unknown[] };
  export function searchText(
    handle: unknown,
    pageIndex: number,
    query: string,
  ): unknown[];
  export function applyRedactions(handle: unknown, plan: unknown): unknown;
  export function savePdf(handle: unknown): Uint8Array;
}

