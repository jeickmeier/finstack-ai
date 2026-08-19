#!/usr/bin/env python3
"""Generate the checked-in document fixture corpus. Deterministic output."""
from __future__ import annotations

import io
import zipfile
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "fixtures" / "documents"


def build_pdf(object_bodies: list[bytes], root_obj_num: int = 1) -> bytes:
    """Assemble a minimal but structurally valid PDF: header, numbered
    objects, a byte-accurate xref table, and a trailer pointing at it.

    `lopdf` (via pdf-inspector) requires a real xref table with correct
    offsets — a bare `trailer` with no `xref`/`startxref` is rejected as
    "Invalid PDF structure", which hand-written fixtures without this
    builder produced.
    """
    header = b"%PDF-1.4\n"
    body = bytearray(header)
    offsets: list[int] = [0]  # object 0 is the free-list head
    for index, obj_body in enumerate(object_bodies, start=1):
        offsets.append(len(body))
        body += f"{index} 0 obj\n".encode() + obj_body + b"\nendobj\n"

    xref_offset = len(body)
    count = len(object_bodies) + 1
    xref = bytearray(f"xref\n0 {count}\n".encode())
    xref += b"0000000000 65535 f \n"
    for offset in offsets[1:]:
        xref += f"{offset:010d} 00000 n \n".encode()

    trailer = (
        f"trailer\n<</Size {count}/Root {root_obj_num} 0 R>>\n"
        f"startxref\n{xref_offset}\n%%EOF\n"
    ).encode()

    return bytes(body) + bytes(xref) + trailer


def _text_pdf() -> bytes:
    content = b"BT /F1 24 Tf 72 720 Td (Quarterly Revenue Report) Tj ET"
    return build_pdf([
        b"<</Type/Catalog/Pages 2 0 R>>",
        b"<</Type/Pages/Kids[3 0 R]/Count 1>>",
        b"<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]"
        b"/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>",
        f"<</Length {len(content)}>>\nstream\n".encode() + content + b"\nendstream",
        b"<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>",
    ])


def _scanned_pdf() -> bytes:
    # A one-page PDF whose only content is a 1x1 image XObject: no text
    # operators, so pdf-inspector classifies it as scanned/needs-OCR.
    content = b"q 612 0 0 792 0 0 cm /Im1 Do Q"
    image_data = b"\xff"
    return build_pdf([
        b"<</Type/Catalog/Pages 2 0 R>>",
        b"<</Type/Pages/Kids[3 0 R]/Count 1>>",
        b"<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]"
        b"/Contents 4 0 R/Resources<</XObject<</Im1 5 0 R>>>>>>",
        f"<</Length {len(content)}>>\nstream\n".encode() + content + b"\nendstream",
        f"<</Type/XObject/Subtype/Image/Width 1/Height 1/ColorSpace/DeviceGray"
        f"/BitsPerComponent 8/Length {len(image_data)}>>\nstream\n".encode()
        + image_data
        + b"\nendstream",
    ])


TEXT_PDF = _text_pdf()
SCANNED_PDF = _scanned_pdf()

CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"""

ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"""

DOCUMENT_XML = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body><w:p><w:r><w:t>Hello from a docx fixture.</w:t></w:r></w:p></w:body></w:document>"""

XLSX_CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"""

XLSX_ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"""

WORKBOOK_XML = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"""

WORKBOOK_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"""

SHEET_XML = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Revenue</t></is></c><c r="B1"><v>1250</v></c></row></sheetData></worksheet>"""


def zip_bytes(parts: dict[str, str]) -> bytes:
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, content in parts.items():
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            archive.writestr(info, content)
    return buffer.getvalue()


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "text.pdf").write_bytes(TEXT_PDF)
    (OUT / "scanned.pdf").write_bytes(SCANNED_PDF)
    (OUT / "sample.docx").write_bytes(zip_bytes({
        "[Content_Types].xml": CONTENT_TYPES,
        "_rels/.rels": ROOT_RELS,
        "word/document.xml": DOCUMENT_XML,
    }))
    (OUT / "sample.xlsx").write_bytes(zip_bytes({
        "[Content_Types].xml": XLSX_CONTENT_TYPES,
        "_rels/.rels": XLSX_ROOT_RELS,
        "xl/workbook.xml": WORKBOOK_XML,
        "xl/_rels/workbook.xml.rels": WORKBOOK_RELS,
        "xl/worksheets/sheet1.xml": SHEET_XML,
    }))
    (OUT / "sample.pptx").write_bytes(zip_bytes({
        "[Content_Types].xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
<Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
</Types>""",
        "_rels/.rels": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>""",
        "ppt/presentation.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>""",
        "ppt/_rels/presentation.xml.rels": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>""",
        "ppt/slides/slide1.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/>
<p:txBody><a:bodyPr/><a:p><a:r><a:t>Slide fixture title</a:t></a:r></a:p></p:txBody></p:sp>
</p:spTree></p:cSld></p:sld>""",
    }))
    (OUT / "sample.csv").write_bytes(b"quarter,revenue\nQ1,1250\nQ2,1310\n")
    (OUT / "corrupt.bin").write_bytes(b"\x00\x01corrupt-not-a-document\x02\x03")
    print(f"wrote fixtures to {OUT}")


if __name__ == "__main__":
    main()
