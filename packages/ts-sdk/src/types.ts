/** Two-dimensional point in normalized page-space PDF units. */
export type Point = { x: number; y: number };

/** Rectangle redaction target. */
export type RectTarget = {
  kind: "rect";
  pageIndex: number;
  x: number;
  y: number;
  width: number;
  height: number;
};

/** Single quad redaction target. */
export type QuadTarget = {
  kind: "quad";
  pageIndex: number;
  points: [Point, Point, Point, Point];
};

/** Multi-quad redaction target for visual text selections or grouped matches. */
export type QuadGroupTarget = {
  kind: "quadGroup";
  pageIndex: number;
  quads: Array<[Point, Point, Point, Point]>;
};

/** Canonical redaction target union accepted by the engine. */
export type RedactionTarget = RectTarget | QuadTarget | QuadGroupTarget;

/**
 * Controls the visual output of text redaction.
 *
 * - `"strip"` — physically remove bytes; surviving text shifts, no overlay.
 * - `"redact"` — replace bytes with blank space, draw colored overlay. **(default)**
 * - `"erase"` — replace bytes with blank space, no overlay.
 */
export type RedactionMode = "strip" | "redact" | "erase";

/** Redaction plan passed to the apply pipeline. */
export type RedactionPlan = {
  targets: RedactionTarget[];
  mode?: RedactionMode;
  fillColor?: { r: number; g: number; b: number };
  overlayText?: string | null;
  removeIntersectingAnnotations?: boolean;
  stripMetadata?: boolean;
  stripAttachments?: boolean;
};

/** Concrete fill color type derived from the plan shape. */
export type FillColor = NonNullable<RedactionPlan["fillColor"]>;

/** Normalized page size in PDF user-space units. */
export type PageSize = { width: number; height: number };

/** Extracted text item with geometry. */
export type TextItem = {
  text: string;
  bbox: { x: number; y: number; width: number; height: number };
  quad?: [Point, Point, Point, Point];
  charStart?: number;
  charEnd?: number;
};

/** Extracted text and geometry for a page. */
export type PageText = {
  pageIndex: number;
  text: string;
  items: TextItem[];
};

/** Search result returned in visual glyph order. */
export type TextMatch = {
  text: string;
  pageIndex: number;
  quads: Array<[Point, Point, Point, Point]>;
};

/** Summary of work performed by a redaction apply pass. */
export type ApplyReport = {
  pagesTouched: number;
  textGlyphsRemoved: number;
  pathPaintsRemoved: number;
  imageDrawsRemoved: number;
  annotationsRemoved: number;
  /** Number of Form XObject per-page copies produced by copy-on-write redaction. */
  formXObjectsRewritten: number;
  warnings: string[];
};
