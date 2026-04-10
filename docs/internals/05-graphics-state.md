# Graphics State and Coordinate Systems

This document explains how this engine represents and transforms coordinates across the multiple spaces that PDF defines. Incorrect coordinate handling was the source of several real bugs; understanding this document is required before modifying any geometry, matrix, or content-stream code.

---

## 1. Coordinate spaces in PDF

PDF content exists in several nested coordinate spaces that must be traversed in order to produce page-space geometry.

**User space** is the coordinate system in which a content stream is written. The origin is conventionally at the bottom-left of the page, with the y-axis pointing up. All drawing operators (`Td`, `Tm`, glyph advances, path commands) are expressed in user space.

**Device space** is the final output coordinate system — pixels on screen, dots on a printer. The CTM maps user space to device space.

**The CTM (Current Transformation Matrix)** is the accumulated product of all `cm` operators encountered since the beginning of the content stream (or since the most recent `q`). It maps points from user space into device space (or, for this engine's purposes, into the normalized page space described below).

**Text space** is a local coordinate system for glyph placement. It is defined by the text matrix (`Tm`), which is relative to user space. When a glyph is rendered, its local rectangle is first expressed in text space, then transformed by the text matrix, then by the CTM.

**Normalized page space** is this engine's canonical output space. It has its origin at the corner of the crop box and has the page rotation already applied, so coordinates are always in a consistent top-left or bottom-left frame regardless of the PDF's internal orientation. All `TextGlyph` quads, match rects, and redaction targets are expressed in this space.

---

## 2. Matrix representation

All coordinate transforms in this engine use a single `Matrix` type:

```rust
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}
```

This represents the 3×3 affine matrix (in row-major, row-vector form):

```
[ a  b  0 ]
[ c  d  0 ]
[ e  f  1 ]
```

Source: `crates/pdf_graphics/src/geometry.rs`.

The six-element form `[a, b, c, d, e, f]` is exactly the form used in PDF's `cm` operator and in `Tm`.

---

## 3. Row-vector convention

PDF uses **row-vector convention**: a point is represented as the row vector `[x, y, 1]` and is transformed by right-multiplying by the matrix:

```
[x', y', 1] = [x, y, 1] * M
```

Expanding:

```
x' = x*a + y*c + e
y' = x*b + y*d + f
```

This is what `Matrix::transform_point` implements. The convention is chosen to match the PDF specification directly, so the formulas in the spec map one-to-one to the code without transposition.

---

## 4. Matrix multiplication order — the critical decision

`A.multiply(B)` computes the standard matrix product `A * B`.

Because of the row-vector convention, `A.multiply(B).transform_point(p)` is equivalent to:

```
p * (A * B) = (p * A) * B
```

That is: **A is applied first, then B**. The rightmost matrix in the chain is applied last.

### The `cm` operator

Per the PDF specification, the `cm` operator **pre-multiplies** the new matrix onto the existing CTM:

```
CTM' = M_new * CTM_old
```

In code this is:

```rust
ctm = matrix.multiply(ctm);  // correct: pre-multiply
// NOT: ctm = ctm.multiply(matrix);  // wrong: post-multiply
```

### Why the order matters

Consider a content stream where:

- `cm [0.24, 0, 0, -0.24, 0, 841.92]` scales and flips the coordinate space to fit the page (M1).
- Inside a `q`/`Q` block: `cm [3.125, 0, 0, 3.125, 0, 0]` scales up (M2).

**Correct (pre-multiply):** `CTM = M2 * M1`

A point is transformed by M2 first (scale up in local space), then by M1 (scale and flip to page space). Net result: coordinates land in page space.

**Wrong (post-multiply):** `CTM = M1 * M2`

A point is transformed by M1 first (scale down to page), then by M2 (scale up again). Net result: coordinates are roughly 3× outside page bounds.

This exact bug occurred and was fixed in commit `a520542`. The symptom was glyph Y coordinates of approximately 2583 points on a page that is 842 points tall.

The rule is simple: **always pre-multiply when applying `cm`**.

---

## 5. Graphics state stack

The `q` operator saves a snapshot of the graphics state. `Q` restores it.

In `pdf_text`, the full text state is saved alongside the CTM because the PDF specification includes the text state as part of the graphics state:

```rust
ctm_stack.push((ctm, text_state.clone()));
// ...
let (saved_ctm, saved_text_state) = ctm_stack.pop().unwrap();
ctm = saved_ctm;
text_state = saved_text_state;
```

In `pdf_redact`, only the CTM is pushed, because vector and image operations do not need the text state:

```rust
ctm_stack.push(ctm);
// ...
ctm = ctm_stack.pop().unwrap();
```

Both implementations use `Vec` as the stack, which guarantees LIFO order and deterministic behavior.

---

## 6. Page transform

`PageBox::normalized_transform()` produces a matrix that converts from PDF user space into normalized page space. It applies three steps in order:

1. **Translate** the crop box origin to (0, 0) — removes any crop box offset from the page origin.
2. **Apply page rotation** — PDF pages can be rotated 0, 90, 180, or 270 degrees. The rotation matrix is applied around the new origin.
3. **Fix-up translation** — after rotation the origin may have moved; a second translation brings it back to (0, 0).

The result is a single matrix that callers multiply into their transform chain. No caller needs to reason about crop boxes or rotation independently.

---

## 7. The full glyph transform chain

A glyph's local rectangle is expressed in a unit coordinate system local to the glyph. To reach normalized page space it passes through three matrices in sequence:

```
glyph_local_rect → text_matrix → CTM → page_transform → page-space quad
```

In code, the three matrices are pre-composed into a single transform before iterating over glyphs:

```rust
let text_to_page = text_state.text_matrix.multiply(ctm).multiply(page_transform);
```

This is computed once per text-showing operation and then applied to each glyph's local rectangle. The composed matrix is then discarded; only the individual matrices are stored in state.

---

## 8. Why it was coded this way

**Row-vector convention matches the PDF spec directly.** All formulas in the spec (ISO 32000) use row vectors. Adopting the same convention means the code is a direct translation of the spec, not a transposed version of it.

**Single `Matrix` type for all transforms.** CTM, text matrix, and page transform are all represented as `Matrix`. There is no separate `AffineTransform` or `Transform2D` type for different contexts. This reduces the number of conversion sites where bugs can hide.

**`PageBox::normalized_transform()` centralizes the crop/rotate logic.** Every caller that needs to convert from user space to page space calls this one function. If the logic changes (e.g., to handle a new rotation convention), it changes in exactly one place.

---

## 9. What would break

| Change | Consequence |
|---|---|
| `cm` changed to post-multiply | All PDFs with nested `cm` operators produce coordinates outside page bounds. Y values on a 842pt page appear near 2583pt. |
| Text state not saved with `q` | Fonts set before `q` are lost after `Q`; fonts set inside `q` leak out after `Q`. |
| Page transform not applied | Rotated pages produce coordinates in an unrotated frame. Quads are in the wrong quadrant of the page. |
| Vec replaced with HashMap for state stacks | HashMap has no ordering; stack semantics are undefined. Vec is correct and must be preserved. |

---

## 10. Example walkthrough

Consider the following content stream fragment:

```
0.24 0 0 -0.24 0 841.92 cm     % M1: scale to page, flip y
q
  3.125 0 0 3.125 0 0 cm       % M2: scale up inside q block
  BT
    /F1 12 Tf
    100 200 Td
    (Hello) Tj
  ET
Q
```

**Step 1: Parse M1**

```
M1 = { a:0.24, b:0, c:0, d:-0.24, e:0, f:841.92 }
CTM = M1
```

**Step 2: `q` — push CTM**

```
ctm_stack = [M1]
CTM = M1  (unchanged)
```

**Step 3: Parse M2, pre-multiply**

```
M2 = { a:3.125, b:0, c:0, d:3.125, e:0, f:0 }
CTM = M2.multiply(M1)
    = { a:0.75, b:0, c:0, d:-0.75, e:0, f:841.92 }
```

Verification: a point at local (100, 200) in user space:

```
x' = 100*0.75 + 200*0   + 0     = 75
y' = 100*0    + 200*(-0.75) + 841.92 = 691.92
```

That is within the 0–842 pt page height. Correct.

**Step 4: `Td 100 200` — advance text matrix**

```
text_matrix = identity.translate(100, 200)
```

**Step 5: Show glyph "H" (example advance 7.2pt at 12pt)**

```
advance = (722/1000) * 12 = 8.664
local_rect = { x:0, y:-1.44, width:8.664, height:9.6 }
text_to_page = text_matrix.multiply(CTM).multiply(page_transform)
quad = local_rect.to_quad().transform(text_to_page)
```

**Step 6: `Q` — pop CTM**

```
CTM = M1  (restored)
ctm_stack = []
```

After `Q`, subsequent operators use M1 only. The nested M2 has no effect on anything outside the `q`/`Q` block.
