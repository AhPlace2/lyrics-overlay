# Draws lyrics-overlay.ico: the tray icon (main.rs) as a smooth icon, a blue rounded square with three white
# lines, on its 32-unit grid with 4x4 samples per pixel. Needs Pillow. Run: python make-icon.py
from PIL import Image

BLUE = (0x48, 0xB6, 0xFF)
GRID, SS, SIZE = 32, 4, 256
BARS = [(9, 13, 8, 24), (15, 19, 8, 21), (21, 25, 8, 16)]  # y0, y1, x0, x1, as in main.rs


def sample(x, y):
    dx = max(7 - x, x - (GRID - 7), 0)
    dy = max(7 - y, y - (GRID - 7), 0)
    if dx * dx + dy * dy > 49:
        return None
    white = any(y0 <= y < y1 and x0 <= x < x1 for y0, y1, x0, x1 in BARS)
    return (255, 255, 255) if white else BLUE


img = Image.new("RGBA", (SIZE, SIZE))
px = img.load()
for py in range(SIZE):
    for qx in range(SIZE):
        r = g = b = n = 0
        for sy in range(SS):
            for sx in range(SS):
                c = sample((qx + (sx + 0.5) / SS) / SIZE * GRID, (py + (sy + 0.5) / SS) / SIZE * GRID)
                if c:
                    r, g, b, n = r + c[0], g + c[1], b + c[2], n + 1
        px[qx, py] = (round(r / n), round(g / n), round(b / n), round(255 * n / SS**2)) if n else (0, 0, 0, 0)

sizes = [(s, s) for s in (16, 20, 24, 32, 40, 48, 64, 128, 256)]
img.save("lyrics-overlay.ico", sizes=sizes, bitmap_format="bmp")
