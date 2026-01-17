#!/bin/bash

# Generate macOS icon.icns from icon_1024.png
# Usage: ./mac_icon_gen.sh [input_png] [output_icns]
# Defaults: input_png = icon_1024.png, output_icns = icon.icns

INPUT_PNG="${1:-icon_1024.png}"
OUTPUT_ICNS="${2:-icon.icns}"
ICONSET_DIR="$(basename "$OUTPUT_ICNS" .icns).iconset"

# Check if input file exists
if [ ! -f "$INPUT_PNG" ]; then
    echo "Error: Input file '$INPUT_PNG' not found"
    exit 1
fi

# Create iconset directory
mkdir -p "$ICONSET_DIR"

# Generate icon files at all required sizes
echo "Generating icon sizes..."
sips -z 16 16     "$INPUT_PNG" --out "$ICONSET_DIR/icon_16x16.png"
sips -z 32 32     "$INPUT_PNG" --out "$ICONSET_DIR/icon_16x16@2x.png"
sips -z 32 32     "$INPUT_PNG" --out "$ICONSET_DIR/icon_32x32.png"
sips -z 64 64     "$INPUT_PNG" --out "$ICONSET_DIR/icon_32x32@2x.png"
sips -z 128 128   "$INPUT_PNG" --out "$ICONSET_DIR/icon_128x128.png"
sips -z 256 256   "$INPUT_PNG" --out "$ICONSET_DIR/icon_128x128@2x.png"
sips -z 256 256   "$INPUT_PNG" --out "$ICONSET_DIR/icon_256x256.png"
sips -z 512 512   "$INPUT_PNG" --out "$ICONSET_DIR/icon_256x256@2x.png"
sips -z 512 512   "$INPUT_PNG" --out "$ICONSET_DIR/icon_512x512.png"
sips -z 1024 1024 "$INPUT_PNG" --out "$ICONSET_DIR/icon_512x512@2x.png"

# Convert iconset to icns
echo "Converting to $OUTPUT_ICNS..."
iconutil -c icns "$ICONSET_DIR" -o "$OUTPUT_ICNS"

# Remove iconset folder
echo "Cleaning up..."
rm -rf "$ICONSET_DIR"

echo "✓ Done! Created $OUTPUT_ICNS"