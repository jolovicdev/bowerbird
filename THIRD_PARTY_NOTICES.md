# Third-party notices

Bowerbird is under the MIT license (`LICENSE`). Its releases also contain
software and a font by others, listed here with their licenses, and the app
downloads models on first use. License texts are in `licenses/`; PDFium's are
in `pdfium/` beside it (`vendor/pdfium/` in a source checkout, after
`scripts/fetch-pdfium.sh`). Each release carries this file and those folders.

## In every release

**PDFium**, build chromium/7881 from github.com/bblanchon/pdfium-binaries.
PDFium is under the BSD-3-Clause license, Copyright 2014 The PDFium Authors;
the build is under the MIT license, Copyright 2014-2025 Benoit Blanchon
(`pdfium/LICENSE`). PDFium contains Abseil, Anti-Grain Geometry 2.3,
fast_float, FreeType, ICU, Little CMS, libjpeg-turbo, OpenJPEG, libpng,
LibTIFF, LLVM libc, simdutf and zlib, and on Android also catapult,
cpu_features and libunwind, each under its own license
(`pdfium/licenses/`).

- This software is based in part on the work of the FreeType Team.
- This software is based in part on the work of the Independent JPEG Group.

**ONNX Runtime 1.28.0**, linked into the app through the `ort` crate: MIT
license, Copyright (c) Microsoft Corporation
(`licenses/onnxruntime-LICENSE.txt`), with its third-party notices
(`licenses/onnxruntime-ThirdPartyNotices.txt`).

**whisper.cpp 1.8.3 and ggml**, compiled into the app through `whisper-rs`:
MIT license, Copyright (c) 2023-2024 The ggml authors
(`licenses/whisper.cpp-LICENSE.txt`). ggml's CPU code includes code under
the MIT license, Copyright (c) 2023 Jeffrey Quesnelle and Bowen Peng.

**Speech data in loqui-g2p 0.4.0**: the misaki lexicons and English G2P
rules and the PeterReid grapheme-to-phoneme models, under the Apache License
2.0, and the spaCy English tokenizer rules, under the MIT license, Copyright
(c) 2016-2026 ExplosionAI GmbH, 2016 spaCy GmbH, 2015 Matthew Honnibal
(`licenses/loqui-g2p-NOTICE.txt`, `licenses/Apache-2.0.txt`).

**Oniguruma**, compiled in through the `onig` crate: BSD-2-Clause license,
Copyright (c) 2002-2021 K.Kosako (`licenses/oniguruma-COPYING.txt`).

**SQLite**, compiled in through `rusqlite`: public domain.

**Literata**, the typeface of the app: SIL Open Font License 1.1, Copyright
2017 The Literata Project Authors (`licenses/Literata-OFL-1.1.txt`).

**pdf.js 6.4.299**, which shows PDF pages: Apache License 2.0, Copyright 2024
Mozilla Foundation (`licenses/Apache-2.0.txt`).

**Rust crates and npm packages.** `licenses/rust-crates.txt` lists every
crate compiled into the app and `licenses/ui-packages.txt` every npm package
bundled into its web pages, each with its license text.

## In the Windows release

**DirectML 1.15.4** (`DirectML.dll`): Microsoft Software License Terms for
DirectML (`licenses/DirectML-LICENSE.txt`), with its third-party notices
(`licenses/DirectML-ThirdPartyNotices.txt`).

**WebView2 loader**, linked into `Bowerbird.exe`: BSD-3-Clause license,
Copyright (C) Microsoft Corporation (`licenses/WebView2-LICENSE.txt`).

## In the Android release

**libc++** from the Android NDK: Apache License 2.0 with LLVM Exceptions
(`licenses/libcxx-LICENSE.txt`).

**AndroidX, Material Components for Android and the Kotlin standard
library**: Apache License 2.0 (`licenses/Apache-2.0.txt`).

## In the Linux AppImage

The AppImage carries the WebKitGTK, GTK, GLib, libsoup and GStreamer
libraries of the Ubuntu 24.04 machine that built it, unmodified, under their
own licenses, mostly the GNU LGPL version 2 or 2.1 or later. Their source is
in the Ubuntu 24.04 source packages of the same versions (for example
`apt-get source libwebkit2gtk-4.1-0`). The `.deb` uses the system's copies
instead.

## Models downloaded on first use

No release contains these. The app fetches each from Hugging Face or GitHub
the first time a feature needs it.

- **EmbeddingGemma 300M**, 4-bit ONNX (onnx-community/embeddinggemma-300m-ONNX),
  for search by meaning. Gemma is provided under and subject to the Gemma
  Terms of Use found at ai.google.dev/gemma/terms. Its use is subject to the
  restrictions in section 3.2 of those terms, including the Gemma Prohibited
  Use Policy at ai.google.dev/gemma/prohibited_use_policy.
- **PP-OCRv5** mobile text detection and Latin recognition, from PaddleOCR,
  for reading scanned PDFs: Apache License 2.0.
- **Kokoro-82M v1.0** by hexgrad, for audio overviews: Apache License 2.0.
- **Whisper small** by OpenAI, converted for whisper.cpp, for transcribing
  recordings: MIT license, Copyright (c) 2022 OpenAI.
