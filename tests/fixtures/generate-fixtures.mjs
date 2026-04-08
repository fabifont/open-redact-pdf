import fs from "node:fs";
import path from "node:path";

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
