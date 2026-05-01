import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";

const fixturesDir = path.resolve("tests/fixtures");

function pdfString(value) {
  return `(${value.replaceAll("\\", "\\\\").replaceAll("(", "\\(").replaceAll(")", "\\)")})`;
}

// Encode `text` (interpreted as Latin-1 bytes) with the RunLengthDecode
// filter (PDF § 7.4.5). This is a "plain" encoder — it emits only literal
// runs of at most 128 bytes followed by the 128 EOD marker, which keeps
// the fixture easy to reason about while still exercising the filter's
// literal-run path end-to-end.
function encodeRunLength(text) {
  const bytes = Buffer.from(text, "binary");
  let out = "";
  let offset = 0;
  while (offset < bytes.length) {
    const run = Math.min(128, bytes.length - offset);
    out += String.fromCharCode(run - 1);
    for (let i = 0; i < run; i += 1) out += String.fromCharCode(bytes[offset + i]);
    offset += run;
  }
  out += String.fromCharCode(128); // EOD
  return out;
}

// Encode `text` (interpreted as Latin-1 bytes) with the TIFF-compatible LZW
// variant used by PDF streams: 9–12-bit codes, 256 = CLEAR, 257 = EOD,
// default /EarlyChange = 1 (width bumps one code earlier). The Rust decoder
// in crates/pdf_objects/src/stream.rs mirrors this encoder's width logic.
function encodeLzw(text) {
  const dict = new Map();
  for (let byte = 0; byte < 256; byte += 1) {
    dict.set(String.fromCharCode(byte), byte);
  }
  let nextCode = 258;
  let codeWidth = 9;
  const bits = [];
  const emit = (code, width) => {
    for (let i = width - 1; i >= 0; i -= 1) {
      bits.push((code >> i) & 1);
    }
  };
  emit(256, 9);
  let buffer = "";
  for (const ch of text) {
    const extended = buffer + ch;
    if (dict.has(extended)) {
      buffer = extended;
      continue;
    }
    emit(dict.get(buffer), codeWidth);
    dict.set(extended, nextCode);
    nextCode += 1;
    if (nextCode >= (1 << codeWidth) - 1 && codeWidth < 12) {
      codeWidth += 1;
    }
    buffer = ch;
  }
  if (buffer.length > 0) {
    emit(dict.get(buffer), codeWidth);
  }
  emit(257, codeWidth);
  while (bits.length % 8 !== 0) bits.push(0);
  let binary = "";
  for (let i = 0; i < bits.length; i += 8) {
    let byte = 0;
    for (let j = 0; j < 8; j += 1) byte = (byte << 1) | bits[i + j];
    binary += String.fromCharCode(byte);
  }
  return binary;
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

// StandardEncoding fixture: Helvetica simple font with `/Encoding
// /StandardEncoding`. Adobe Standard Encoding maps 0x27 to `quoteright`
// (U+2019) and 0x60 to `quoteleft` (U+2018) rather than the ASCII
// apostrophe and grave accent. This fixture encodes "Don't" + grave + "e"
// so that a correct StandardEncoding path yields "Don\u{2019}t\u{2018}e".
writeFixture("standard-encoding.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: { Font: { F1: { ref: [6, 0] } } },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        // Hex bytes: 446F6E = "Don", 27 = quoteright, 74 = "t",
        // 60 = quoteleft, 65 = "e".
        data: "BT\n/F1 24 Tf\n72 700 Td\n<446F6E27746065> Tj\nET\n",
      },
    },
    fontObject,
    {
      id: 6,
      value: {
        Type: "/Font",
        Subtype: "/Type1",
        BaseFont: "/Helvetica",
        Encoding: "/StandardEncoding",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// MacRomanEncoding fixture: Helvetica simple font with `/Encoding
// /MacRomanEncoding`. The content stream uses a PDF hex string to
// embed bytes that only make sense under Mac Roman — e.g. 0xD2/0xD3
// decode to U+201C / U+201D (curly double quotes) under Mac Roman
// but to the Private Use Area under WinAnsi.
writeFixture("mac-roman-encoding.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: { Font: { F1: { ref: [6, 0] } } },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        // Hex string bytes: 48656C6C6F20 = "Hello ", D2 = U+201C,
        // 576F726C64 = "World", D3 = U+201D.
        data: "BT\n/F1 24 Tf\n72 700 Td\n<48656C6C6F20D2576F726C64D3> Tj\nET\n",
      },
    },
    fontObject,
    {
      id: 6,
      value: {
        Type: "/Font",
        Subtype: "/Type1",
        BaseFont: "/Helvetica",
        Encoding: "/MacRomanEncoding",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Pathologically dense layout: 8pt font with rows only 1pt apart. Sits
// right at the boundary of the absolute 1pt y-tolerance cap; regression
// guard against future tolerance tweaks that would collapse such rows.
writeFixture("ultra-dense-layout.pdf", {
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
          "BT\n/F1 8 Tf\n" +
          "72 700 Td (Row Alpha 1000) Tj\n" +
          "0 -1 Td (Row Beta 2000) Tj\n" +
          "0 -1 Td (Row Gamma 3000) Tj\n" +
          "ET\n",
      },
    },
    fontObject,
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Dense layout: small font, very tight leading between rows so adjacent
// baselines sit ~2pt apart. Exercises the visual-line grouping heuristic
// under conditions where a naive y-tolerance could merge rows.
writeFixture("dense-layout.pdf", {
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
        // Each line is placed at explicit absolute coordinates using Td,
        // 2 pt apart. Helvetica 6pt keeps glyph bounding boxes small enough
        // that without the absolute y-tolerance cap the rows would merge.
        data:
          "BT\n/F1 6 Tf\n" +
          "72 700 Td (Account A 1111) Tj\n" +
          "0 -2 Td (Account B 2222) Tj\n" +
          "0 -2 Td (Account C 3333) Tj\n" +
          "0 -2 Td (Account D 4444) Tj\n" +
          "ET\n",
      },
    },
    fontObject,
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Sub-1pt dense layout: 6pt Helvetica with rows only 0.5pt apart in y.
// Exercises the proportional `height_ref * 0.10` y-tolerance — under the
// previous `min(line_height * 0.3, 1.0)` formula the 1.0pt absolute cap
// merged all three rows into a single visual line.
writeFixture("sub-pt-dense-layout.pdf", {
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
          "BT\n/F1 6 Tf\n" +
          "72 700 Td (Row A 111) Tj\n" +
          "0 -0.5 Td (Row B 222) Tj\n" +
          "0 -0.5 Td (Row C 333) Tj\n" +
          "ET\n",
      },
    },
    fontObject,
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Content stream that uses a BX/EX compatibility section to wrap an
// unrecognized operator (`sh`). Without BX/EX support the engine would
// refuse to redact the page; with it, the unknown op is passed through
// and the surrounding recognized operators are still rewritten normally.
writeFixture("bx-ex-compat.pdf", {
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
        data: "BT\n/F1 24 Tf\n72 700 Td\n(Redact compat sample) Tj\n0 -32 Td\n(Keep alpha) Tj\nET\nBX\n/Pattern1 sh\nEX\n",
      },
    },
    fontObject,
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Content stream compressed with the RunLengthDecode filter. Exercises
// the parser/decoder path for `/Filter /RunLengthDecode` end-to-end.
const runLengthContentStream =
  "BT\n/F1 24 Tf\n72 700 Td\n(Redact RLE sample) Tj\n0 -32 Td\n(Keep alpha) Tj\nET\n";
writeFixture("run-length-content.pdf", {
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
        dict: { Filter: "/RunLengthDecode" },
        data: encodeRunLength(runLengthContentStream),
      },
    },
    fontObject,
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Content stream compressed with the LZW filter. Exercises the
// parser/decoder path for `/Filter /LZWDecode` and the default
// `/EarlyChange 1` setting end-to-end.
const lzwContentStream =
  "BT\n/F1 24 Tf\n72 700 Td\n(Redact LZW sample) Tj\n0 -32 Td\n(Keep alpha) Tj\nET\n";
writeFixture("lzw-content.pdf", {
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
        dict: { Filter: "/LZWDecode" },
        data: encodeLzw(lzwContentStream),
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

// Composite font using Adobe's predefined UCS-2 BE CMap (UniGB-UCS2-H).
// The CMap's character codes ARE Unicode BMP scalars, so no ToUnicode
// stream is required: the engine decodes bytes directly to Unicode.
// Content stream renders "中文" as <4E2D6587>. DW=1000 is the standard
// full-em width for CJK glyphs; W is omitted so every glyph falls back
// to default_width.
writeFixture("type0-ucs2-h.pdf", {
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
        data: "BT\n/F1 24 Tf\n72 700 Td\n<4E2D6587> Tj\nET\n",
      },
    },
    {
      id: 5,
      value: {
        Type: "/Font",
        Subtype: "/Type0",
        BaseFont: "/DemoCJKFont",
        Encoding: "/UniGB-UCS2-H",
        DescendantFonts: [{ ref: [6, 0] }],
      },
    },
    {
      id: 6,
      value: {
        Type: "/Font",
        Subtype: "/CIDFontType2",
        BaseFont: "/DemoCJKFont",
        CIDSystemInfo: {
          Registry: "Adobe",
          Ordering: "GB1",
          Supplement: 5,
        },
        DW: 1000,
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Composite font using Adobe's predefined UTF-16 BE CMap (UniJIS-UTF16-H).
// Exercises the surrogate-pair decode path: the four bytes <D840DC00>
// encode U+20000 (𠀀, an SMP CJK ideograph), followed by <4E2D> for "中"
// in the BMP. No ToUnicode entry; the SMP scalar is composed from the
// raw UTF-16 surrogate pair.
writeFixture("type0-utf16-h.pdf", {
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
        data: "BT\n/F1 24 Tf\n72 700 Td\n<D840DC004E2D> Tj\nET\n",
      },
    },
    {
      id: 5,
      value: {
        Type: "/Font",
        Subtype: "/Type0",
        BaseFont: "/DemoCJKFont",
        Encoding: "/UniJIS-UTF16-H",
        DescendantFonts: [{ ref: [6, 0] }],
      },
    },
    {
      id: 6,
      value: {
        Type: "/Font",
        Subtype: "/CIDFontType2",
        BaseFont: "/DemoCJKFont",
        CIDSystemInfo: {
          Registry: "Adobe",
          Ordering: "Japan1",
          Supplement: 6,
        },
        DW: 1000,
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Vector path that uses the v and y Bezier curve operators (single-control
// shorthand for c). The path traces a filled curved shape directly under the
// text, so that a redaction target on the text forces the engine to
// neutralize both the text and the underlying curve fill.
writeFixture("vector-vy-curves.pdf", {
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
          "0 0 1 rg\n" +
          // Curved shape: m → v (curve through current point) → y (curve to endpoint)
          "80 690 m\n" +
          "240 760 80 760 v\n" +
          "240 700 240 690 y\n" +
          "h f\n" +
          "BT\n/F1 20 Tf\n100 710 Td\n(Curve Secret) Tj\nET\n",
      },
    },
    fontObject,
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

// Simple-font /Encoding dictionary with /BaseEncoding /WinAnsiEncoding and a
// /Differences array that overrides a handful of bytes with glyph names that
// must be resolved through the Adobe Glyph List. Byte 0x40 (normally '@' in
// WinAnsi) is overridden to /AE → Æ, and byte 0x7B (normally '{') to
// /fi → ﬁ (ligature).
writeFixture("encoding-differences.pdf", {
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
        // Bytes:  0x40 0x20 0x7B 0x20 "nice"
        data: "BT\n/F1 24 Tf\n72 700 Td\n(@ { nice) Tj\nET\n",
      },
    },
    {
      id: 5,
      value: {
        Type: "/Font",
        Subtype: "/Type1",
        BaseFont: "/Helvetica",
        Encoding: { ref: [6, 0] },
      },
    },
    {
      id: 6,
      value: {
        Type: "/Encoding",
        BaseEncoding: "/WinAnsiEncoding",
        Differences: [64, "/AE", 123, "/fi"],
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Document with Optional Content Groups where one layer is off by default.
// Redaction must refuse this file because hidden-layer content cannot be
// safely targeted through the visible glyph list.
writeFixture("ocg-hidden-layer.pdf", {
  objects: [
    {
      id: 1,
      value: {
        Type: "/Catalog",
        Pages: { ref: [2, 0] },
        OCProperties: {
          OCGs: [{ ref: [7, 0] }],
          D: {
            Order: [{ ref: [7, 0] }],
            OFF: [{ ref: [7, 0] }],
          },
        },
      },
    },
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
        data: "BT\n/F1 20 Tf\n72 700 Td\n(Visible Line) Tj\nET\n",
      },
    },
    fontObject,
    {
      id: 7,
      value: {
        Type: "/OCG",
        Name: "Hidden Layer",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Document with an Optional Content Group that is off by default AND the
// content stream actually carries a `BDC /OC /Hidden ... EMC` block
// referencing it. The /OFF array entry marks the layer as hidden, so the
// default rejection path refuses this file. With `sanitize_hidden_ocgs: true`
// set on the plan, the sanitization pass strips the hidden block before the
// rest of redaction runs, leaving the visible line intact.
writeFixture("ocg-hidden-content.pdf", {
  objects: [
    {
      id: 1,
      value: {
        Type: "/Catalog",
        Pages: { ref: [2, 0] },
        OCProperties: {
          OCGs: [{ ref: [7, 0] }],
          D: {
            Order: [{ ref: [7, 0] }],
            OFF: [{ ref: [7, 0] }],
          },
        },
      },
    },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: {
        Font: { F1: { ref: [5, 0] } },
        Properties: { Hidden: { ref: [7, 0] } },
      },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data:
          "BT\n/F1 20 Tf\n72 700 Td\n(Visible Line) Tj\nET\n" +
          "/OC /Hidden BDC\n" +
          "BT\n/F1 20 Tf\n72 670 Td\n(Hidden Secret) Tj\nET\n" +
          "EMC\n",
      },
    },
    fontObject,
    {
      id: 7,
      value: {
        Type: "/OCG",
        Name: "Hidden Layer",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Same fixture shape as ocg-hidden-content.pdf but the default
// configuration uses `/BaseState /OFF`, which hides every OCG unless
// it is explicitly listed under `/ON`. Tests that the sanitization
// pass recognises the BaseState form and still strips hidden content.
writeFixture("ocg-base-state-off.pdf", {
  objects: [
    {
      id: 1,
      value: {
        Type: "/Catalog",
        Pages: { ref: [2, 0] },
        OCProperties: {
          OCGs: [{ ref: [7, 0] }],
          D: {
            Order: [{ ref: [7, 0] }],
            BaseState: "/OFF",
          },
        },
      },
    },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: {
        Font: { F1: { ref: [5, 0] } },
        Properties: { Hidden: { ref: [7, 0] } },
      },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data:
          "BT\n/F1 20 Tf\n72 700 Td\n(Visible Line) Tj\nET\n" +
          "/OC /Hidden BDC\n" +
          "BT\n/F1 20 Tf\n72 670 Td\n(Hidden Secret) Tj\nET\n" +
          "EMC\n",
      },
    },
    fontObject,
    {
      id: 7,
      value: {
        Type: "/OCG",
        Name: "Hidden Layer",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Form XObject fixture. Page text is split between the page content stream
// ("Page Outer") and a referenced Form XObject ("Form Inner Secret"). The
// Form has its own Matrix (translating the inner text by +100 in y) and its
// own Font resource (F2) to prove that Form-local resources are used while
// still inheriting any unmentioned names from the parent.
writeFixture("form-xobject-text.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: {
        Font: { F1: { ref: [5, 0] } },
        XObject: { Fm1: { ref: [6, 0] } },
      },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data:
          "BT\n/F1 18 Tf\n72 750 Td\n(Page Outer) Tj\nET\n" +
          "q\n1 0 0 1 72 400 cm\n/Fm1 Do\nQ\n",
      },
    },
    fontObject,
    {
      id: 6,
      stream: {
        dict: {
          Type: "/XObject",
          Subtype: "/Form",
          FormType: 1,
          BBox: [0, 0, 400, 200],
          Matrix: [1, 0, 0, 1, 0, 100],
          Resources: { Font: { F2: { ref: [7, 0] } } },
        },
        data: "BT\n/F2 14 Tf\n0 0 Td\n(Form Inner Secret) Tj\nET\n",
      },
    },
    {
      id: 7,
      value: {
        Type: "/Font",
        Subtype: "/Type1",
        BaseFont: "/Helvetica",
        Encoding: "/WinAnsiEncoding",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Nested Form XObject: the page does Do /FmOuter which itself does Do
// /FmInner. The inner Form carries the targeted text "Nested Secret".
// Tests that the copy-on-write redactor recurses through the outer Form
// into the inner one, rewrites the inner content, and repoints the
// outer Form's own Resources.XObject at the redacted inner copy.
writeFixture("form-xobject-nested.pdf", {
  objects: [
    { id: 1, value: { Type: "/Catalog", Pages: { ref: [2, 0] } } },
    { id: 2, value: { Type: "/Pages", Count: 1, Kids: [{ ref: [3, 0] }] } },
    basePageObjects({
      pageId: 3,
      pagesId: 2,
      contentId: 4,
      resources: {
        Font: { F1: { ref: [5, 0] } },
        XObject: { FmOuter: { ref: [6, 0] } },
      },
    }),
    {
      id: 4,
      stream: {
        dict: {},
        data: "BT\n/F1 18 Tf\n72 750 Td\n(Page Outer) Tj\nET\n" + "q\n1 0 0 1 0 0 cm\n/FmOuter Do\nQ\n",
      },
    },
    fontObject,
    {
      id: 6,
      stream: {
        dict: {
          Type: "/XObject",
          Subtype: "/Form",
          FormType: 1,
          BBox: [0, 0, 612, 792],
          Matrix: [1, 0, 0, 1, 0, 0],
          Resources: {
            Font: { F2: { ref: [7, 0] } },
            XObject: { FmInner: { ref: [8, 0] } },
          },
        },
        data: "BT\n/F2 12 Tf\n72 600 Td\n(Middle Layer) Tj\nET\n" + "q\n1 0 0 1 0 0 cm\n/FmInner Do\nQ\n",
      },
    },
    {
      id: 7,
      value: {
        Type: "/Font",
        Subtype: "/Type1",
        BaseFont: "/Helvetica",
        Encoding: "/WinAnsiEncoding",
      },
    },
    {
      id: 8,
      stream: {
        dict: {
          Type: "/XObject",
          Subtype: "/Form",
          FormType: 1,
          BBox: [0, 0, 612, 792],
          Matrix: [1, 0, 0, 1, 0, 0],
          Resources: { Font: { F3: { ref: [9, 0] } } },
        },
        data: "BT\n/F3 14 Tf\n72 500 Td\n(Nested Secret) Tj\nET\n",
      },
    },
    {
      id: 9,
      value: {
        Type: "/Font",
        Subtype: "/Type1",
        BaseFont: "/Helvetica",
        Encoding: "/WinAnsiEncoding",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

// Single-byte Type1 font declaring /Encoding /WinAnsiEncoding so that
// bytes like 0xC9, 0xE0, 0xB0, 0x92, 0x80 decode to É, à, °, ’, € instead
// of replacement characters. The content stream shows a string built
// directly from those WinAnsi byte values.
writeFixture("winansi-font.pdf", {
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
        // (Caff\xC9 50\xB0 \x80 l\x92anno) — WinAnsi encoding of "Caffé 50° € l’anno"
        data: "BT\n/F1 24 Tf\n72 700 Td\n(Caff\xC9 50\xB0 \x80 l\x92anno) Tj\nET\n",
      },
    },
    {
      id: 5,
      value: {
        Type: "/Font",
        Subtype: "/Type1",
        BaseFont: "/Helvetica",
        Encoding: "/WinAnsiEncoding",
      },
    },
  ],
  trailer: { Root: { ref: [1, 0] } },
});

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

// xref-stream form WITHOUT object streams: every indirect object stays
// uncompressed but the cross-reference is encoded as a `/Type /XRef`
// stream rather than a classic xref table. Used by the writer round-trip
// test to confirm that input shape is mirrored on save even when no
// ObjStm packing is involved.
function buildXrefStreamNoObjstmPdf() {
  // Indirect object ids:
  //   1: Catalog
  //   2: Pages
  //   3: Page
  //   4: content stream
  //   5: Font
  //   6: XRef stream

  const catalogBody = "<< /Type /Catalog /Pages 2 0 R >>";
  const pagesBody = "<< /Type /Pages /Count 1 /Kids [3 0 R] >>";
  const pageBody =
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] " +
    "/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>";
  const contentData =
    "BT\n/F1 24 Tf\n72 700 Td\n(Plain XRef Stream) Tj\nET\n";
  const fontBody = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";

  let body = "%PDF-1.5\n%\xFF\xFF\xFF\xFF\n";

  const obj1Offset = Buffer.byteLength(body, "binary");
  body += `1 0 obj\n${catalogBody}\nendobj\n`;
  const obj2Offset = Buffer.byteLength(body, "binary");
  body += `2 0 obj\n${pagesBody}\nendobj\n`;
  const obj3Offset = Buffer.byteLength(body, "binary");
  body += `3 0 obj\n${pageBody}\nendobj\n`;
  const obj4Offset = Buffer.byteLength(body, "binary");
  body +=
    `4 0 obj\n<< /Length ${Buffer.byteLength(contentData, "binary")} >>\nstream\n${contentData}endstream\nendobj\n`;
  const obj5Offset = Buffer.byteLength(body, "binary");
  body += `5 0 obj\n${fontBody}\nendobj\n`;

  // Build xref stream entries with W = [1 3 1].
  // Object 0 is the head of the free list; objects 1..5 are uncompressed.
  const row = (t, a, b) =>
    Buffer.from([t, (a >> 16) & 0xff, (a >> 8) & 0xff, a & 0xff, b & 0xff]);
  const entries = Buffer.concat([
    row(0, 0, 0),
    row(1, obj1Offset, 0),
    row(1, obj2Offset, 0),
    row(1, obj3Offset, 0),
    row(1, obj4Offset, 0),
    row(1, obj5Offset, 0),
    row(1, 0, 0), // placeholder, replaced after we know obj6Offset
  ]);

  const obj6Offset = Buffer.byteLength(body, "binary");
  // Patch the entry for object 6 (the xref stream itself) with its own
  // offset now that we know it. Per ISO 32000-1, the xref stream's own
  // entry uses Type 1 with the stream's offset.
  const last = row(1, obj6Offset, 0);
  last.copy(entries, 6 * 5);

  const xrefStreamDict =
    `<< /Type /XRef /Size 7 /W [1 3 1] /Root 1 0 R /Length ${entries.length} >>`;
  const xrefStreamObj = Buffer.concat([
    Buffer.from(`6 0 obj\n${xrefStreamDict}\nstream\n`, "binary"),
    entries,
    Buffer.from("\nendstream\nendobj\n", "binary"),
  ]);

  const trailer = Buffer.from(`startxref\n${obj6Offset}\n%%EOF\n`, "binary");

  return Buffer.concat([Buffer.from(body, "binary"), xrefStreamObj, trailer]);
}

fs.writeFileSync(
  path.join(fixturesDir, "xref-stream-no-objstm.pdf"),
  buildXrefStreamNoObjstmPdf(),
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
