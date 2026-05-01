---
title: Working with encrypted PDFs
---

# Working with encrypted PDFs

Open Redact PDF parses and decrypts the Standard Security Handler in place during `parse_pdf`. Once the handler authenticates, the engine's downstream stages (text extraction, search, redaction, save) see a plaintext document — the trailer's `/Encrypt` entry is dropped and saved output is always unencrypted.

## Supported configurations

| V | R | Method | Notes |
|---|---|---|---|
| 1 | 2 | RC4-40 | Pre-PDF 1.4 encryption |
| 2 | 3 | RC4-128 | Common "encrypted to prevent editing but openable by anyone" |
| 4 | 4 | RC4-128 via `/CFM /V2` | V=4 crypt filter choosing RC4 |
| 4 | 4 | AES-128-CBC via `/CFM /AESV2` | Default for modern Acrobat / Preview save-with-password |
| 5 | 5 | AES-256-CBC via `/CFM /AESV3` | PDF 1.7 Extension Level 3 (vulnerable hash — still decryptable) |
| 5 | 6 | AES-256-CBC via `/CFM /AESV3` | ISO 32000-2 iterative Algorithm 2.B hash |

Either the user password or the owner password authenticates. The empty user password is accepted as a special case without any caller action.

The public-key handler (`/Filter /Adobe.PubSec`) is also supported for the modern SubFilters:

| SubFilter | V | Inner cipher | Notes |
|---|---|---|---|
| `adbe.pkcs7.s4` | 4 | AES-128-CBC | RSA-PKCS1v15 or RSA-OAEP recipient unwrap |
| `adbe.pkcs7.s5` | 5 | AES-256-CBC | RSA-PKCS1v15 or RSA-OAEP recipient unwrap |

PubSec PDFs are decrypted by supplying a recipient X.509 certificate (DER) plus its matching RSA private key (DER, PKCS#8). Recipients are matched by `IssuerAndSerialNumber` (issuer + serial equality) or `SubjectKeyIdentifier`.

Unsupported configurations (`adbe.pkcs7.s3` V=1 RC4-40, `KeyAgreeRecipientInfo` ECDH recipients, non-AES-CBC inner ciphers, `/CFM` methods outside the tables above) still fail up front with `PdfError::Unsupported`.

## API

### Rust facade

```rust
use open_redact_pdf::PdfDocument;

// Empty-password (or unencrypted) documents.
let document = PdfDocument::open(&bytes)?;

// Documents that need a non-empty user or owner password.
let document = PdfDocument::open_with_password(&bytes, b"secret")?;

// Public-key encrypted (Adobe.PubSec): supply DER-encoded cert and key.
let document = PdfDocument::open_with_certificate(
    &pdf_bytes,
    &recipient_cert_der,
    &recipient_private_key_der,
)?;
```

A wrong password or unrelated certificate surfaces `PdfError::InvalidPassword`; wrap with `Result::or_else` to prompt a retry.

### TypeScript / WebAssembly SDK

```ts
import {
  initWasm,
  openPdf,
  openPdfWithPassword,
  openPdfWithCertificate,
} from "@fabifont/open-redact-pdf";

await initWasm();

try {
  const handle = openPdf(bytes);
  /* ... */
} catch (caught) {
  if (caught instanceof Error && /invalid password/i.test(caught.message)) {
    const handle = openPdfWithPassword(bytes, await promptForPassword());
    /* ... */
  } else {
    throw caught;
  }
}

// Public-key encrypted PDFs require a recipient certificate + private key,
// both DER-encoded. Convert PEM / PKCS#12 to DER on the JS side first
// (e.g. via `subtle.exportKey("pkcs8", key)` for the private key).
const certDer = new Uint8Array(/* ...DER bytes of recipient cert... */);
const keyDer = new Uint8Array(/* ...DER bytes of PKCS#8 private key... */);
const pubsecHandle = openPdfWithCertificate(pdfBytes, certDer, keyDer);
```

The password string is sent to the engine as UTF-8 bytes. Non-ASCII passwords round-trip as their UTF-8 byte representation. The certificate and private-key buffers are passed through to the WASM decryption path and dropped on completion — the SDK never persists or transmits them.

## Demo flow

The browser demo under `apps/demo-web` shows this pattern end-to-end: when `openPdf` reports `invalid password`, an inline password form appears, and submitting it calls `openPdfWithPassword` with the entered string. A wrong submission re-renders the form with a `Password did not authenticate. Try again.` hint; cancelling returns the app to its initial state.

## Output

The save path is always unencrypted — `PdfDocument::save` emits a plaintext deterministic rewrite regardless of whether the input was encrypted. Re-encrypting the sanitized output is intentionally out of scope: the redaction tool's job is to remove targeted content, not to gate downstream access. Callers who need an encrypted output can pipe the plaintext save through an external tool (e.g. `qpdf --encrypt`).

## Security notes

- Decryption happens once at parse time, before object streams are materialized. The plaintext document is kept entirely in memory.
- The trailer's `/Encrypt` entry is dropped after decryption so no stage downstream can re-observe the ciphertext.
- `/EncryptMetadata false` on V=4 documents is honoured: the Algorithm 2 step-5 `0xFFFFFFFF` suffix is applied to the file key and `/Type /Metadata` streams are left in plaintext (they were never encrypted in the source).
- V=5 never mixes per-object keys — the 32-byte file key is used directly for every `/AESV3` string and stream.

## Related docs

- [Security model](../security-model.md)
- [Supported PDF subset](../reference/supported-subset.md)
- [Rust API](../reference/rust-api.md)
- [TypeScript SDK](../reference/ts-sdk.md)
