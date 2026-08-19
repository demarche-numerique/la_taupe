#!/bin/sh
# Fetches the OCR models embedded in the binary at build time (include_bytes!).
# Run automatically by build.rs in release mode when a model is missing or empty.
# The resulting binary is self-contained: nothing is downloaded at runtime.
set -e

# PP-OCR v6 tiny (PaddleOCR, ONNX), from the oar-ocr registry on ModelScope.
# SHA-256 are those published in oar-ocr-core/src/core/download/registry.rs.
PPOCR_BASE="https://www.modelscope.cn/api/v1/models/greatv/oar-ocr/repo?Revision=master&FilePath="

fetch_checked() {
  name="$1"; sha="$2"
  curl -fsSL "${PPOCR_BASE}${name}" -o "models/${name}"
  echo "${sha}  models/${name}" | sha256sum -c --quiet
}

fetch_checked pp-ocrv6_tiny_det.onnx 193bab7a04fca699a6c82e6abb5b81bdb28177f0abd4062552b04908dafb19f8
fetch_checked pp-ocrv6_tiny_rec.onnx 9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6
fetch_checked ppocrv6_dict.txt       b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d
