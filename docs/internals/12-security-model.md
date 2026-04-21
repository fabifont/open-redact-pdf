# Security and Correctness Model

## 1. Core security principle

A visible black rectangle is not a redaction unless the underlying content is removed or neutralized at the byte level.

## 2. What this engine guarantees

- Targeted text bytes are physically removed from or replaced in the content stream
- In **Redact** mode: kern compensation preserves layout, and an overlay covers the resulting gap
- In **Strip/Erase** modes: bytes are removed without overlay
- Metadata and attachments can be stripped
- Output is a single-revision PDF — no old content is accessible via a `Prev` chain
- `FileAttachment` annotations are always removed regardless of their position

## 3. What this engine does NOT guarantee

- Complete redaction of all copies of targeted content (text may appear in bookmarks, outlines, or destinations not parsed by this engine)
- Redaction of content inside Form XObjects (hard error if present on targeted pages)
- Redaction of content in unsupported font encodings
- Protection against PDF recovery or forensics on the original file

## 4. Defensive design choices

- **Operator whitelist**: unknown operators on redacted pages cause hard errors rather than silently passing through
- **Explicit unsupported errors**: encrypted PDFs, unsupported stream filters or predictors, nested object streams, non-Identity-H encodings, Form XObjects that intersect targets, and documents with off-by-default Optional Content Groups all fail explicitly
- **Decompression bomb protection**: 256 MiB limit on decoded stream size
- **Page tree depth limit**: `MAX_PAGE_TREE_DEPTH = 64` prevents stack overflow from malformed trees
- **Cycle detection**: applied in page tree traversal, `Prev` chain following, and reachable-ref collection
- **Conservative annotation removal**: annotations without a `Rect` are removed (except Links)

## 5. The "fail explicitly" philosophy

Every unsupported feature returns `PdfError::Unsupported` or `PdfError::UnsupportedOption`. The engine never silently degrades. This is critical for redaction: silent degradation could mean unredacted content passes through to the output file without the caller being aware.

## 6. Encrypted PDFs

The Standard Security Handler is parsed and consumed at parse time so every stage downstream operates on plaintext objects. Supported configurations:

| V | R | Method | Notes |
|---|---|---|---|
| 1 | 2 | RC4-40 | Algorithm 2 + 4 |
| 2 | 3 | RC4-128 | Algorithm 2 + 5 (50-round rehash) |
| 4 | 4 | RC4-128 via `/StdCF /CFM /V2` | Algorithm 1 per-object key, no `sAlT` suffix |
| 4 | 4 | AES-128-CBC via `/StdCF /CFM /AESV2` | Algorithm 1a per-object key with `sAlT` suffix, PKCS#7-padded, 16-byte IV prepended |

Either the user password or the owner password authenticates. The owner password is recovered to the user password via Algorithm 7; the file key is always derived from the user password.

`/Identity` crypt filters are pass-through — bytes are returned unchanged without touching the cipher.

When a V=4 document sets `/EncryptMetadata false`:

- file-key derivation appends `0xFFFFFFFF` after the `/ID[0]` bytes (Algorithm 2 step 5)
- streams with `/Type /Metadata` skip decryption so they stay readable as plaintext XMP

Unsupported encryption configurations (V=5/R=6 AES-256, public-key handlers, `/CFM` methods other than `/V2` and `/AESV2`) fail explicitly with `PdfError::Unsupported`. Wrong passwords fail with `PdfError::InvalidPassword`.

Writing encrypted PDFs is out of scope: the save path always emits a plaintext, deterministic full-save rewrite.

## 7. Known security-relevant limitations

- **`v` and `y` bezier curves**: path bounds may be underestimated because these curves are not fully accumulated
- **Quad intersection uses AABB approximation**: for rotated quads, narrow slivers may be missed
- **No ToUnicode for simple fonts**: non-ASCII text in Type1/TrueType fonts appears as replacement characters and cannot be searched or redacted by text search
- **Text in invisible mode (`Tr=3`)**: included in glyphs for redaction but excluded from search results — this is correct behavior, since you must be able to redact what you cannot see

## 8. Why it was coded this way

- **Whitelist over blacklist**: an unknown operator might carry redactable content; passing it through blindly is unsafe
- **Fail-explicit over fail-soft**: for a redaction tool, silent failure is a security vulnerability, not a graceful degradation
- **Conservative annotation removal**: an annotation without geometric overlap may still contain sensitive information in its metadata

## 9. What would break

| Change | Consequence |
|---|---|
| Switching to an operator blacklist | Unknown operators pass through; potential data leak |
| Allowing Form XObjects to pass through | Content inside them escapes redaction |
| Not stripping `Prev` from saved files | Entire pre-redaction document accessible via `Prev` chain |
| Not removing `FileAttachment` annotations | Attached files survive redaction intact |
