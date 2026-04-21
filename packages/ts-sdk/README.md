# @fabifont/open-redact-pdf

Typed TypeScript and WebAssembly SDK for `open-redact-pdf`.

## Install

```bash
npm install @fabifont/open-redact-pdf
```

## Basic usage

```ts
import { initWasm, openPdf, getPageCount } from "@fabifont/open-redact-pdf";

await initWasm();
const handle = openPdf(pdfBytes);
const count = getPageCount(handle);
```

For the full API and integration guidance, see:

- https://fabifont.github.io/open-redact-pdf/reference/ts-sdk/
- https://fabifont.github.io/open-redact-pdf/guides/browser-integration/
