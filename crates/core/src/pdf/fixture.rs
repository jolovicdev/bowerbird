//! Writes minimal valid PDFs (Helvetica text in WinAnsi encoding, so
//! `\374` is ü; optional outline) for tests.

pub struct Page<'a> {
    /// (font size, y position, text)
    pub lines: Vec<(f32, f32, &'a str)>,
}

pub fn build(pages: &[Page], outline: &[(&str, usize)]) -> Vec<u8> {
    let mut objects: Vec<String> = Vec::new();
    let page_count = pages.len();
    // 1 catalog, 2 pages, 3 font, then per page: page + content, then outline objects.
    let first_page_obj = 4;
    let outline_root = first_page_obj + page_count * 2;
    let catalog = if outline.is_empty() {
        "<< /Type /Catalog /Pages 2 0 R >>".to_string()
    } else {
        format!("<< /Type /Catalog /Pages 2 0 R /Outlines {outline_root} 0 R >>")
    };
    objects.push(catalog);
    let kids: Vec<String> = (0..page_count)
        .map(|i| format!("{} 0 R", first_page_obj + i * 2))
        .collect();
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {page_count} >>",
        kids.join(" ")
    ));
    objects.push(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_string(),
    );
    for (i, page) in pages.iter().enumerate() {
        let content_obj = first_page_obj + i * 2 + 1;
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {content_obj} 0 R >>"
        ));
        let mut stream = String::new();
        for (size, y, text) in &page.lines {
            stream.push_str(&format!("BT /F1 {size} Tf 72 {y} Td ({text}) Tj ET\n"));
        }
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}endstream",
            stream.len()
        ));
    }
    if !outline.is_empty() {
        let first_item = outline_root + 1;
        let last_item = outline_root + outline.len();
        objects.push(format!(
            "<< /Type /Outlines /First {first_item} 0 R /Last {last_item} 0 R /Count {} >>",
            outline.len()
        ));
        for (i, (title, page)) in outline.iter().enumerate() {
            let obj = first_item + i;
            let mut item = format!(
                "<< /Title ({title}) /Parent {outline_root} 0 R /Dest [{} 0 R /XYZ 0 792 0]",
                first_page_obj + page * 2
            );
            if i > 0 {
                item.push_str(&format!(" /Prev {} 0 R", obj - 1));
            }
            if i + 1 < outline.len() {
                item.push_str(&format!(" /Next {} 0 R", obj + 1));
            }
            item.push_str(" >>");
            objects.push(item);
        }
    }

    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref_at = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A PDF of page images with no text layer, like a scanned book: each
/// page is (RGB bytes, width, height), drawn over a letter-size page.
pub fn scanned(pages: &[(Vec<u8>, u32, u32)]) -> Vec<u8> {
    // 1 catalog, 2 pages, then per page: page, content, image.
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            (0..pages.len())
                .map(|i| format!("{} 0 R", 3 + i * 3))
                .collect::<Vec<_>>()
                .join(" "),
            pages.len()
        )
        .into_bytes(),
    ];
    for (i, (rgb, width, height)) in pages.iter().enumerate() {
        let (content, image) = (4 + i * 3, 5 + i * 3);
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Im0 {image} 0 R >> >> /Contents {content} 0 R >>"
        ).into_bytes());
        let draw = "q 612 0 0 792 0 0 cm /Im0 Do Q";
        objects.push(
            format!(
                "<< /Length {} >>\nstream\n{draw}\nendstream",
                draw.len() + 1
            )
            .into_bytes(),
        );
        let mut xobject = format!(
            "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n",
            rgb.len()
        )
        .into_bytes();
        xobject.extend_from_slice(rgb);
        xobject.extend_from_slice(b"\nendstream");
        objects.push(xobject);
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_at = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}
