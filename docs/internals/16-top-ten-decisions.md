# Top 10 Implementation Decisions Every Maintainer Must Understand

This page documents ten decisions embedded in the codebase that are easy to misread as bugs or oversights, but are deliberate and load-bearing. Each entry explains what the decision is, why it was made, and what would break if you changed it without understanding the reasoning.

---

## 1. Matrix Multiplication Order for `cm`: Pre-Multiply

**What:** The `cm` operator handler concatenates the new matrix onto the current transformation matrix (CTM) using pre-multiplication:

```
ctm = matrix.multiply(ctm)
```

**Why:** The PDF specification (ISO 32000 8.3.4) defines CTM concatenation as pre-multiplication. The incoming `cm` operand is a transformation applied *before* the existing CTM in the coordinate chain, not after. In matrix notation: `CTM_new = M_cm × CTM_old`.

**What breaks if you reverse it:** Post-multiplication (`ctm = ctm.multiply(matrix)`) produces correct results only for a flat (non-nested) transform sequence. As soon as content has nested `q`/`cm`/`Q` blocks — which is common for rotated, scaled, or positioned content — glyphs are placed outside the page bounds. This produced incorrect bounding quads for all affected text runs. (Fixed in commit a520542.)

---

## 2. BT Only Resets the Text Matrix, Not the Full Text State

**What:** The `BT` (begin text) operator handler resets `text_matrix` and `text_line_matrix` to the identity matrix, but does not reset font, font size, character spacing, word spacing, text rise, rendering mode, or any other text state parameter.

**Why:** The PDF specification (ISO 32000 9.4.1) is explicit: BT initializes the text matrix and text line matrix. It does not reset the text state. Parameters set before `BT` — including the font set via an `ExtGState` `gs` operator — remain in effect inside the BT/ET block.

**What breaks if you reset the full text state in BT:** Any font installed via a `gs` operator before the `BT` block is discarded. The engine then operates with no valid font reference for that text block, producing either garbled character decoding or a panic. This was the root cause of the font-loss bug fixed in commit 5eb0043.

---

## 3. `q`/`Q` Saves and Restores the Full Text State

**What:** The graphics state stack push (`q`) and pop (`Q`) operators save and restore not only the CTM and graphics parameters, but also the entire text state (font, size, spacing values, rendering mode, text matrix).

**Why:** The PDF specification (ISO 32000 8.4.2) defines the graphics state as including the text state. A `q`/`Q` pair is therefore required to save and restore all of it. Content generators frequently set a font inside a `q` block and rely on the font being fully restored (or discarded) on `Q`.

**What breaks if you save/restore only the CTM:** The font reference and other text state parameters leak across `q`/`Q` boundaries. A font set inside a `q` block persists after the `Q`, corrupting decoding for all subsequent text operations. Conversely, a font set before a `q` block may be overwritten and not restored on `Q`. Both failure modes produce incorrect text extraction and potentially incorrect redaction. (Fixed in commit 5eb0043.)

---

## 4. Search Index Uses Byte-Indexed `normalized_to_display` Mapping

**What:** The visual search index maps positions in the normalized (lowercased, whitespace-collapsed) search string back to positions in the display string using byte offsets, not character (codepoint) offsets. The mapping is built with `str::find()`, which returns byte offsets.

**Why:** Rust string indexing is byte-based. `str::find()` returns a byte offset. If the mapping used per-character (codepoint) indices, any multi-byte UTF-8 character in the display string would cause all subsequent mapping entries to be shifted by the difference between the character count and the byte count. For ASCII-only text the two are identical, which is why the bug was not caught earlier.

**What breaks if you switch to character indices:** For any display string containing multi-byte UTF-8 characters (accented Latin, Greek, Cyrillic, CJK, etc.), every search match after the first multi-byte character maps to the wrong glyph run. The resulting redaction quads are displaced and may cover the wrong text or nothing at all. (Fixed in commit fc85fcf.)

---

## 5. Newest Xref Entry Wins (`or_insert`, Not `insert`)

**What:** When building the xref map from an incremental-update chain (following `Prev` pointers from newest to oldest revision), object entries are added to the map using `or_insert` semantics: an entry is only inserted if no entry for that object number already exists.

**Why:** Incremental updates are traversed newest-first (the most recent xref table is processed first). `or_insert` means the first entry encountered — the newest — wins. If `insert` were used (unconditionally overwriting), each revision would overwrite the previous, and the oldest revision's entries would ultimately win. This is exactly backwards: the oldest revision holds the pre-redaction or pre-edit state of objects, while the newest revision holds the current state.

**What breaks if you use `insert`:** For any PDF with incremental updates, object lookups resolve to the oldest available version of each object rather than the current one. Redaction targets the wrong content, font references point to stale objects, and the output does not reflect the document as the user sees it. (Implemented in commit c98fb90.)

---

## 6. Operator Allowlist (Not Blocklist) on Redacted Pages

**What:** When rewriting a content stream for a page that has active redaction targets, the engine uses an explicit allowlist of known-safe PDF operators. Operators not on the allowlist are dropped from the output stream.

**Why:** A blocklist approach — drop only operators known to carry visible content, pass everything else — cannot be made safe. PDF has a large and extensible operator set, and private or future operators might carry text or image data. The allowlist approach guarantees that only operators whose behavior is fully understood survive into the redacted output.

**What breaks if you switch to a blocklist:** An unknown operator in the input stream would pass through into the output unchanged. If that operator renders text, images, or other content under the redaction target, the content survives redaction despite the black rectangle overlay. This is a security vulnerability, not merely a visual artifact. The correct failure mode for an unknown operator on a redacted page is to drop it.

---

## 7. Object Removal Is Deferred Until All Pages Are Processed

**What:** The pipeline collects all indirect object references to be removed (orphaned streams, superseded font descriptors, etc.) and performs the removal pass after all pages have been fully processed and rewritten.

**Why:** PDF documents frequently share indirect objects across pages. A Form XObject used as a logo, a font shared by multiple pages, or a resource dictionary referenced from several pages are all examples of objects that appear on multiple pages. If removal were applied page-by-page during processing, an object removed after page 1 might still be needed by page 3. The deferred approach treats the full document as a unit.

**What breaks if you remove objects eagerly during page processing:** Any shared object removed while processing an earlier page becomes unavailable when processing a later page that references it. The result ranges from missing resources (invisible text, missing images) to panics or errors when the engine attempts to dereference the removed object.

---

## 8. Output Is Always Single-Revision (`Prev` and `XRefStm` Removed)

**What:** The writer always emits a single-revision PDF. The `Prev` pointer in the trailer dictionary and any `XRefStm` key are never written to the output. The output contains exactly one xref table and one set of objects.

**Why:** This is a security requirement, not merely a simplification. A PDF with incremental updates contains multiple revisions, each a complete snapshot of the document at a point in time. If the pre-redaction revision is preserved in the output via the `Prev` chain, any PDF reader that can navigate incremental updates — which all conforming readers can — can reconstruct the original, unredacted content. Single-revision output eliminates this attack surface entirely.

**What breaks if you preserve `Prev`:** The redacted PDF is not actually redacted. A reader following the `Prev` pointer chain retrieves the original object versions, including the unredacted content streams. The black rectangles are visible in the default view, but the underlying text is recoverable. This is a fundamental security failure.

---

## 9. Invisible Text (Tr=3) Is Included in Glyphs but Excluded from Search

**What:** Glyphs rendered with text rendering mode 3 (invisible, `Tr=3`) are included in the glyph list returned by `analyze_page_text` and are therefore targetable by geometry-based redaction. However, they are excluded from the normalized text used for substring search.

**Why:** Text rendering mode 3 is the standard mechanism for OCR overlay layers: the text is positioned over a scanned image and carries the searchable/selectable content, but is not intended to be visible to the reader. Such text must be redactable — if it is not, the OCR content survives redaction and the document leaks information through copy-paste or accessibility tools even though nothing is visually apparent. At the same time, including invisible text in search results would produce matches that appear to return nothing when highlighted, confusing users. The asymmetry is intentional: geometry-based targeting covers everything, search covers only what is visible.

**What breaks if you exclude Tr=3 glyphs entirely:** OCR text survives redaction. A document with a scanned page and an invisible OCR layer appears correctly redacted (black rectangles visible) but all OCR text is still present and extractable by any tool that reads the PDF text layer.

**What breaks if you include Tr=3 in search:** Search returns matches that have no visible highlight and no apparent location on the page. Users cannot confirm what was matched, making the search experience misleading.

---

## 10. Deterministic Output: `BTreeMap` Throughout

**What:** All associative data structures in the serialization and object model paths use `BTreeMap` rather than `HashMap`. This applies to the object table, resource dictionaries, and any other map that contributes to the output byte stream.

**Why:** `HashMap` in Rust uses a randomized hash seed (SipHash with a random initial state) by default. This means that iteration order over a `HashMap` differs between runs, between compilations, and between platforms. A PDF serializer that iterates over a `HashMap` to write dictionary entries produces output that is binary-different on every run, even for identical input. This makes output files non-reproducible, test fixtures non-stable (byte-level comparison fails intermittently), and debugging output diffs meaningless.

**What breaks if you switch to `HashMap`:** Integration tests that compare output byte-for-byte against fixture files become flaky — they pass sometimes and fail sometimes depending on hash seed initialization. Reproducible builds are impossible. Debugging a regression requires distinguishing real changes from noise introduced by key ordering variation. The `BTreeMap` constraint is inexpensive (PDF documents do not contain millions of keys) and the stability guarantee it provides is essential for a correct test suite.
