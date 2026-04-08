export type Point = { x: number; y: number };

export type RectTarget = {
  kind: "rect";
  pageIndex: number;
  x: number;
  y: number;
  width: number;
  height: number;
};

export type QuadTarget = {
  kind: "quad";
  pageIndex: number;
  points: [Point, Point, Point, Point];
};

export type QuadGroupTarget = {
  kind: "quadGroup";
  pageIndex: number;
  quads: Array<[Point, Point, Point, Point]>;
};

export type RedactionTarget = RectTarget | QuadTarget | QuadGroupTarget;

export type RedactionPlan = {
  targets: RedactionTarget[];
  fillColor?: { r: number; g: number; b: number };
  overlayText?: string | null;
  removeIntersectingAnnotations?: boolean;
  stripMetadata?: boolean;
  stripAttachments?: boolean;
};

export type FillColor = NonNullable<RedactionPlan["fillColor"]>;

export type PageSize = { width: number; height: number };

export type TextItem = {
  text: string;
  bbox: { x: number; y: number; width: number; height: number };
  quad?: [Point, Point, Point, Point];
  charStart?: number;
  charEnd?: number;
};

export type PageText = {
  pageIndex: number;
  text: string;
  items: TextItem[];
};

export type TextMatch = {
  text: string;
  pageIndex: number;
  quads: Array<[Point, Point, Point, Point]>;
};

export type ApplyReport = {
  pages_touched: number;
  text_glyphs_removed: number;
  path_paints_removed: number;
  image_draws_removed: number;
  annotations_removed: number;
  warnings: string[];
};
