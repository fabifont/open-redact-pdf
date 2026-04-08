# @open-redact-pdf/sdk

Typed TypeScript and WebAssembly SDK for `open-redact-pdf`.

## Install

```bash
npm install @open-redact-pdf/sdk
```

## Basic usage

```ts
import { initWasm, openPdf, getPageCount } from "@open-redact-pdf/sdk";

await initWasm();
const handle = openPdf(pdfBytes);
const count = getPageCount(handle);
```

For the full API and integration guidance, see:

- https://fabifont.github.io/open-redact-pdf/reference/ts-sdk.html
- https://fabifont.github.io/open-redact-pdf/guides/browser-integration.html
