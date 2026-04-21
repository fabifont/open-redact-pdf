import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";

const fixturesDir = path.resolve("tests/fixtures");

function pdfString(value) {
  return `(${value.replaceAll("\\", "\\\\").replaceAll("(", "\\(").replaceAll(")", "\\)")})`;
}

function serializeValue(value) {
  if (value === null) return "null";
  if (typeof value === "number") return Number.isInteger(value) ? String(value) : String(value);
  if (typeof value === "string") return value.startsWith("/") ? value : pdfString(value);
  if (value && value.ref) return `${value.ref[0]} ${value.ref[1]} R`;
  if (Array.isArray(value)) return `[${value.map(serializeValue).join(" ")}]`;
  if (value && value.stream) {
    throw new Error("stream objects must be serialized at the object level");
  }
  if (value && typeof value === "object") {
    return `<< ${Object.entries(value)
      .map(([key, entry]) => `/${key} ${serializeValue(entry)}`)
      .join(" ")} >>`;
  }
  throw new Error(`unsupported value: ${value}`);
}

function buildPdf({ objects, trailer }) {
  let body = "%PDF-1.4\n%\xFF\xFF\xFF\xFF\n";
  const offsets = new Map();
  for (const object of objects) {
    offsets.set(object.id, body.length);
    body += `${object.id} 0 obj\n`;
    if (object.stream) {
      const dict = { ...object.stream.dict, Length: Buffer.byteLength(object.stream.data, "binary") };
      body += `${serializeValue(dict)}\nstream\n${object.stream.data}`;
      if (!object.stream.data.endsWith("\n")) body += "\n";
      body += "endstream\nendobj\n";
    } else {
      body += `${serializeValue(object.value)}\nendobj\n`;
    }
  }
  const startxref = body.length;
  const maxId = Math.max(...objects.map((object) => object.id));
  body += `xref\n0 ${maxId + 1}\n`;
  body += "0000000000 65535 f \n";
  for (let id = 1; id <= maxId; id += 1) {
    const offset = offsets.get(id) ?? 0;
    const flag = offsets.has(id) ? "n" : "f";
    body += `${String(offset).padStart(10, "0")} 00000 ${flag} \n`;
  }
  body += "trailer\n";
  body += `${serializeValue({ Size: maxId + 1, ...trailer })}\n`;
  body += `startxref\n${startxref}\n%%EOF\n`;
  return body;
}

function writeFixture(name, spec) {
  fs.writeFileSync(path.join(fixturesDir, name), buildPdf(spec), "binary");
}

function basePageObjects({ pageId, pagesId, contentId, extraPage = {}, resources, mediaBox = [0, 0, 612, 792] }) {
  return {
    id: pageId,
    value: {
      Type: "/Page",
      Parent: { ref: [pagesId, 0] },
      MediaBox: mediaBox,
      Resources: resources,
      Contents: { ref: [contentId, 0] },
      ...extraPage,
    },
  };
}

const fontObject = {
  id: 5,
  value: {
    Type: "/Font",
    Subtype: "/Type1",
    BaseFont: "/Helvetica",
    Encoding: "/WinAnsiEncoding",
  },
};

// Font set only via gs operator (ExtGState Font entry), no Tf in the content stream
writeFixture("extgstate-font.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: {
        ExtGState: { GS1: { ref: [6, 0] } },
      },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data: "/GS1 gs\nBT\n72 700 Td\n(ExtGState Secret) Tj\n0 -32 Td\n(Normal Line) Tj\nET\n",
      },
    },
    fontObject,
    {
      id: 6,
      value: {
        Type: "/ExtGState",
        Font: [{ ref: [5, 0] }, 24],
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Content stream with inline image (BI/ID/EI) and dictionary operand (BDC with <<...>>)
writeFixture("inline-image.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: { Font: { F1: { ref: [5, 0] } } },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data:
          "/Span <</MCID 0>> BDC\n" +
          "BT\n/F1 20 Tf\n72 700 Td\n(Inline Image Secret) Tj\nET\n" +
          "EMC\n" +
          "BI\n/W 2 /H 2 /CS /G /BPC 8\nID \xFF\xFF\xFF\xFF\nEI\n" +
          "BT\n/F1 20 Tf\n72 660 Td\n(After Image) Tj\nET\n",
      },
    },
    fontObject,
  ],
  trailer: { Root: { ref: [1, 0] } },
});

writeFixture("simple-text.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: { Font: { F1: { ref: [5, 0] } } },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data: "BT\n/F1 24 Tf\n72 700 Td\n(Secret Alpha) Tj\n0 -32 Td\n(Beta Gamma) Tj\nET\n",
      },
    },
    fontObject,
  ],
  trailer: { Root: { ref: [1, 0] } },
});

writeFixture("multi-page.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    {
      id: 2,
      value: { Type: "/Pages", Count: 2, Kids: [{ ref: [3, 0] }, { ref: [6, 0] }] },
    },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: { Font: { F1: { ref: [5, 0] } } },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data: "BT\n/F1 20 Tf\n72 700 Td\n(Page One Secret) Tj\nET\n",
      },
    },
    fontObject,
    basePageObjects({
      pageId: 6,
      pagesId: 2,
      contentId: 7,
      resources: { Font: { F1: { ref: [5, 0] } } },
    }),
    {
      id: 7,
      stream: {
        dict: {},
        data: "BT\n/F1 20 Tf\n72 700 Td\n(Page Two Public) Tj\nET\n",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

writeFixture("rotated-text.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: { Font: { F1: { ref: [5, 0] } } },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data: "BT\n/F1 18 Tf\n0 1 -1 0 200 200 Tm\n(Rotated Secret) Tj\nET\n",
      },
    },
    fontObject,
  ],
  trailer: { Root: { ref: [1, 0] } },
});

writeFixture("type0-search.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: { Font: { F1: { ref: [5, 0] } } },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data: "BT\n/F1 24 Tf\n72 700 Td\n<000100020003000400050006000700080009000A> Tj\nET\n",
      },
    },
    {
      id: 5,
      value: {
        Type: "/Font",
        Subtype: "/Type0",
        BaseFont: "/DemoCIDFont",
        Encoding: "/Identity-H",
        DescendantFonts: [{ ref: [6, 0] }],
        ToUnicode: { ref: [7, 0] },
      },
    },
    {
      id: 6,
      value: {
        Type: "/Font",
        Subtype: "/CIDFontType2",
        BaseFont: "/DemoCIDFont",
        CIDSystemInfo: {
          Registry: "Adobe",
          Ordering: "Identity",
          Supplement: 0,
        },
        DW: 600,
        W: [1, [600, 600, 600, 600, 600, 600, 600, 600, 600, 600]],
      },
    },
    {
      id: 7,
      stream: {
        dict: {},
        data:
          "/CIDInit /ProcSet findresource begin\n" +
          "12 dict begin\n" +
          "begincmap\n" +
          "/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n" +
          "/CMapName /Adobe-Identity-UCS def\n" +
          "/CMapType 2 def\n" +
          "1 begincodespacerange\n" +
          "<0000> <FFFF>\n" +
          "endcodespacerange\n" +
          "10 beginbfchar\n" +
          "<0001> <0053>\n" +
          "<0002> <0065>\n" +
          "<0003> <0063>\n" +
          "<0004> <0072>\n" +
          "<0005> <0065>\n" +
          "<0006> <0074>\n" +
          "<0007> <0020>\n" +
          "<0008> <0043>\n" +
          "<0009> <0049>\n" +
          "<000A> <0044>\n" +
          "endbfchar\n" +
          "endcmap\n" +
          "CMapName currentdict /CMap defineresource pop\n" +
          "end\n" +
          "end\n",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

writeFixture("vector-heavy.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: { Font: { F1: { ref: [5, 0] } } },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data: "0 0 1 rg\n100 600 120 40 re\nf\nBT\n/F1 20 Tf\n72 700 Td\n(Vector Secret) Tj\nET\n",
      },
    },
    fontObject,
  ],
  trailer: { Root: { ref: [1, 0] } },
});

writeFixture("image-xobject.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: {
        Font: { F1: { ref: [5, 0] } },
        XObject: { Im1: { ref: [6, 0] } },
      },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data: "q\n100 0 0 100 72 600 cm\n/Im1 Do\nQ\nBT\n/F1 20 Tf\n72 700 Td\n(Image Secret) Tj\nET\n",
      },
    },
    fontObject,
    {
      id: 6,
      stream: {
        dict: {
          Type: "/XObject",
          Subtype: "/Image",
          Width: 1,
          Height: 1,
          ColorSpace: "/DeviceGray",
          BitsPerComponent: 8,
        },
        data: "\xFF",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

writeFixture("annotations.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: { Font: { F1: { ref: [5, 0] } } },
      extraPage: { Annots: [{ ref: [6, 0] }] },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data: "BT\n/F1 20 Tf\n72 700 Td\n(Annotated Secret) Tj\nET\n",
      },
    },
    fontObject,
    {
      id: 6,
      value: {
        Type: "/Annot",
        Subtype: "/Link",
        Rect: [70, 695, 250, 720],
        Border: [0, 0, 0],
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// --- Incremental update fixture ---
// Builds a two-revision PDF: the original has "Original Secret", then an
// incremental update replaces the content stream with "Updated Secret".
function buildIncrementalPdf() {
  // --- Revision 1: original document ---
  const rev1Spec = {
    objects: [
      { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
      { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
      basePageObjects({
        pageId: 3,
        pagesId: 2,
        contentId: 4,
        resources: { Font: { F1: { ref: [5, 0] } } },
      }),
      {
        id: 4,
        stream: {
          dict: {},
          data: "BT\n/F1 24 Tf\n72 700 Td\n(Original Secret) Tj\nET\n",
        },
      },
      fontObject,
    ],
    trailer: { Root: { ref: [1, 0] } },
  };
  let body = buildPdf(rev1Spec);
  // Remove trailing %%EOF newline for clean append
  if (body.endsWith("\n")) body = body.slice(0, -1);

  // Find the startxref offset of revision 1
  const startxrefMarker = "startxref\n";
  const startxrefPos = body.lastIndexOf(startxrefMarker);
  const afterMarker = startxrefPos + startxrefMarker.length;
  const eofPos = body.indexOf("\n", afterMarker);
  const rev1Xref = body.slice(afterMarker, eofPos);

  // --- Revision 2: incremental update replacing object 4 ---
  const updatedStreamData = "BT\n/F1 24 Tf\n72 700 Td\n(Updated Secret) Tj\nET\n";
  const updatedStreamLength = Buffer.byteLength(updatedStreamData, "binary");

  let rev2Body = "\n";
  const rev2Offset = body.length + 1; // offset of object 4 in the appended body
  rev2Body += `4 0 obj\n<< /Length ${updatedStreamLength} >>\nstream\n${updatedStreamData}endstream\nendobj\n`;

  const rev2XrefOffset = body.length + rev2Body.length;
  rev2Body += "xref\n";
  rev2Body += "0 1\n";
  rev2Body += "0000000000 65535 f \n";
  rev2Body += "4 1\n";
  rev2Body += `${String(rev2Offset).padStart(10, "0")} 00000 n \n`;
  rev2Body += "trailer\n";
  rev2Body += `${serializeValue({ Size: 6, Root: { ref: [1, 0] }, Prev: Number(rev1Xref) })}\n`;
  rev2Body += `startxref\n${rev2XrefOffset}\n%%EOF\n`;

  return body + rev2Body;
}

fs.writeFileSync(path.join(fixturesDir, "incremental-update.pdf"), buildIncrementalPdf(), "binary");

// --- PDF 1.5 xref stream + object stream fixture ---
// Builds a PDF where Catalog, Pages, Page, and Font dictionaries live inside
// an object stream (ObjStm). The content stream cannot be stored inside an
// ObjStm (streams inside ObjStm are disallowed by the spec), so it stays as a
// regular uncompressed indirect object. The cross-reference is emitted as an
// xref stream with `W [1 3 1]`.
function buildXrefObjectStreamPdf() {
  // Indirect object ids:
  //   1: content stream (uncompressed)
  //   2: ObjStm (uncompressed; Flate-compressed body)
  //   3: Catalog (compressed, ObjStm index 0)
  //   4: Pages   (compressed, ObjStm index 1)
  //   5: Page    (compressed, ObjStm index 2)
  //   6: Font    (compressed, ObjStm index 3)
  //   7: XRef stream (uncompressed)

  const catalogBody = "<< /Type /Catalog /Pages 4 0 R >>";
  const pagesBody = "<< /Type /Pages /Count 1 /Kids [5 0 R] >>";
  const pageBody =
    "<< /Type /Page /Parent 4 0 R /MediaBox [0 0 612 792] " +
    "/Resources << /Font << /F1 6 0 R >> >> /Contents 1 0 R >>";
  const fontBody = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";

  const members = [
    { id: 3, body: catalogBody },
    { id: 4, body: pagesBody },
    { id: 5, body: pageBody },
    { id: 6, body: fontBody },
  ];

  // Build ObjStm header: pairs of "obj_num rel_offset" separated by spaces.
  let header = "";
  let runningOffset = 0;
  const relOffsets = [];
  for (const member of members) {
    relOffsets.push(runningOffset);
    header += `${member.id} ${runningOffset} `;
    runningOffset += Buffer.byteLength(member.body, "binary");
  }
  const firstOffset = Buffer.byteLength(header, "binary");
  const decompressed = Buffer.concat([
    Buffer.from(header, "binary"),
    Buffer.from(members.map((m) => m.body).join(""), "binary"),
  ]);
  const compressedObjStm = zlib.deflateSync(decompressed);

  // Content stream (uncompressed for simplicity).
  const contentData =
    "BT\n/F1 24 Tf\n72 700 Td\n(OBJSTM Secret) Tj\n0 -32 Td\n(Beta Gamma) Tj\nET\n";

  let body = "%PDF-1.5\n%\xFF\xFF\xFF\xFF\n";

  const obj1Offset = Buffer.byteLength(body, "binary");
  body +=
    `1 0 obj\n<< /Length ${Buffer.byteLength(contentData, "binary")} >>\nstream\n${contentData}endstream\nendobj\n`;

  const obj2Offset = Buffer.byteLength(body, "binary");
  const objstmDict =
    `<< /Type /ObjStm /N ${members.length} /First ${firstOffset} ` +
    `/Filter /FlateDecode /Length ${compressedObjStm.length} >>`;
  body += `2 0 obj\n${objstmDict}\nstream\n`;
  const preStreamBuf = Buffer.from(body, "binary");
  const afterStreamBuf = Buffer.from("\nendstream\nendobj\n", "binary");

  // Build xref stream body with W = [1 3 1]:
  //   type(1) | field2(3) | field3(1)
  // Entries for objects 0..7:
  //   0: free (type 0, next free = 0, gen 0xFFFF truncated to 0xFF)
  //   1: uncompressed → (1, obj1Offset, 0)
  //   2: uncompressed → (1, obj2Offset, 0)
  //   3..6: compressed in stream 2 at indices 0..3 → (2, 2, index)
  //   7: uncompressed → (1, obj7Offset, 0)
  const row = (t, a, b) => Buffer.from([t, (a >> 16) & 0xff, (a >> 8) & 0xff, a & 0xff, b & 0xff]);

  // We don't know obj7Offset until we've assembled everything up to it.
  // Compose in order: preamble + ObjStm body bytes + afterStream bytes, then obj7.
  const uptoAfterObjStm = Buffer.concat([
    preStreamBuf,
    compressedObjStm,
    afterStreamBuf,
  ]);
  const obj7Offset = uptoAfterObjStm.length;

  const xrefEntries = Buffer.concat([
    row(0, 0, 0),
    row(1, obj1Offset, 0),
    row(1, obj2Offset, 0),
    row(2, 2, 0),
    row(2, 2, 1),
    row(2, 2, 2),
    row(2, 2, 3),
    row(1, obj7Offset, 0),
  ]);

  const xrefStreamDict =
    `<< /Type /XRef /Size 8 /W [1 3 1] /Root 3 0 R /Length ${xrefEntries.length} >>`;
  const obj7 = Buffer.concat([
    Buffer.from(`7 0 obj\n${xrefStreamDict}\nstream\n`, "binary"),
    xrefEntries,
    Buffer.from("\nendstream\nendobj\n", "binary"),
  ]);

  const trailer = Buffer.from(`startxref\n${obj7Offset}\n%%EOF\n`, "binary");

  return Buffer.concat([uptoAfterObjStm, obj7, trailer]);
}

fs.writeFileSync(
  path.join(fixturesDir, "xref-object-stream.pdf"),
  buildXrefObjectStreamPdf(),
);

// Nested cm operators: outer cm scales content space (like many real-world PDFs),
// inner cm inside q/Q scales back up. Tests correct CTM pre-multiplication order.
writeFixture("nested-cm.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      // Page is 612x792 but content space is 2x larger (1224x1584)
      resources: { Font: { F1: { ref: [5, 0] } } },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        // Outer cm scales from 2x content space to page space (0.5 factor)
        // Inner cm inside q/Q scales back up by 2x
        // Net effect on text: coordinates stay within page bounds
        data:
          "0.5 0 0 0.5 0 0 cm\n" +
          "q\n" +
          "2 0 0 2 0 0 cm\n" +
          "BT\n/F1 24 Tf\n72 700 Td\n(Nested CM Secret) Tj\nET\n" +
          "Q\n" +
          "BT\n/F1 48 Tf\n144 1300 Td\n(Outer Text) Tj\nET\n",
      },
    },
    fontObject,
  ],
  trailer: { Root: { ref: [1, 0] } },
});

writeFixture("metadata-attachments.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] }, Names: { ref: [7, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: { Font: { F1: { ref: [5, 0] } } },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data: "BT\n/F1 20 Tf\n72 700 Td\n(Metadata Secret) Tj\nET\n",
      },
    },
    fontObject,
    { id: 6, value: { Producer: "Fixture Generator", Title: "Metadata Fixture" } },
    {
      id: 7,
      value: {
        EmbeddedFiles: { ref: [8, 0] },
      },
    },
    {
      id: 8,
      value: {
        Names: ["note.txt", { ref: [9, 0] }],
      },
    },
    {
      id: 9,
      value: {
        Type: "/Filespec",
        F: "note.txt",
        EF: { F: { ref: [10, 0] } },
      },
    },
    {
      id: 10,
      stream: {
        dict: { Type: "/EmbeddedFile" },
        data: "top secret attachment\n",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] }, Info: { ref: [6, 0] } },
});
