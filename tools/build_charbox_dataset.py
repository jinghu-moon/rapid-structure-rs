# -*- coding: utf-8 -*-
"""Build the character-localization benchmark set (docs/13).

Why this exists: the shipped char boxes come from CTC post-processing (one column
per character + one average width per line), so "is a char box in the right place"
cannot be answered by looking at the picture. This tool renders pages whose
**per-character ink extents are known by construction**, which makes IoU, boundary
error and selection precision/recall computable, and gives Step 3 (forced
alignment) a baseline to be measured against.

Ground truth is derived from the rendered **pixels**, not from font metrics:
for each character, the ink inside its column window (windows split at the midpoint
between neighbouring glyph centres) is the box. Metric ink offsets are only used as
a documented fallback when a window holds no ink, and each char records which route
produced its box (`derived`), so the benchmark can say how much of the GT is pixel
truth rather than metric truth.

Output (all under --out, default datasets/text/charbox):
  pages/pgNN_<group>.png   one page each, white background, black text
  manifest.json            cases + per-line/per-char GT + sha256 of every image

Determinism: no randomness anywhere. Font files, sizes, texts and angles are
explicit, so the manifest's sha256 is the contract the Rust harness verifies.
Rendering is Windows/This-machine specific (font versions), which is why the
dataset is *generated* rather than committed, and why a stale image is an error
instead of a silent re-scoring.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import sys
import unicodedata

from PIL import Image, ImageDraw, ImageFont, ImageOps

FONTS = {
    # key: (file, script)  -- script only drives which texts are legal for it
    "simsun": (r"C:\Windows\Fonts\simsun.ttc", "cn"),
    "simhei": (r"C:\Windows\Fonts\simhei.ttf", "cn"),
    "simkai": (r"C:\Windows\Fonts\simkai.ttf", "cn"),
    "simfang": (r"C:\Windows\Fonts\simfang.ttf", "cn"),
    "arial": (r"C:\Windows\Fonts\arial.ttf", "latin"),
    "times": (r"C:\Windows\Fonts\times.ttf", "latin"),
    "georgia": (r"C:\Windows\Fonts\georgia.ttf", "latin"),
    "tahoma": (r"C:\Windows\Fonts\tahoma.ttf", "latin"),
    "segoeui": (r"C:\Windows\Fonts\segoeui.ttf", "latin"),
    "consolas": (r"C:\Windows\Fonts\consola.ttf", "mono"),
    "lucida": (r"C:\Windows\Fonts\lucon.ttf", "mono"),
    "courier": (r"C:\Windows\Fonts\cour.ttf", "mono"),
}
# msyh.ttc is deliberately absent. Pillow 11.1's FreeType path rasterizes several of
# its CJK glyphs as a truncated bottom band (黑 -> 8px of ink at 30px, 号 -> 1px at
# 20px), identically for every origin, image size, image mode and single-string draw.
# A benchmark whose stimulus is a mangled glyph measures the rasterizer, not the OCR,
# so the font is excluded rather than worked around; check_fidelity() below is what
# keeps it excluded even if someone adds it back.

CN_TEXTS = [
    "多位置文本定位实验",
    "小字号与密集排版",
    "字符边界估计误差",
    "低对比度深色背景",
    "表格与键值对识别",
    "旋转与倾斜文本行",
    "竖排文本传统写法",
    "综合压力测试样本",
]
EN_TEXTS = [
    "Recognition Positioning",
    "Illustration of kerning",
    "narrow illiterate window",
    "Mixed Case Alignment",
    "proportional glyph widths",
    "documentation benchmark",
]
DIGIT_TEXTS = [
    "11223344",
    "2026-10-13",
    "0.75 00 88",
    "1,234,567.89",
    "$98.70 (5:3)",
    "7788 9911",
]
REPEAT_TEXTS = [
    "llllmmmm",
    "WWWiii",
    "OO00ll",
    "112233",
    "nn nn mm",
    "天天密密",
]
MIXED_TEXTS = [
    "OCR 字符定位 2026",
    "batch=64 side=1280",
    "IoU 0.5 / 0.75 阈值",
    "第 3 步 forced alignment",
    "延迟 24.07 s / 892 span",
    "words=chars 档位",
]

PAGE_WIDTH = 1400
PAGE_PAD = 40
LINE_GUTTER = 26
STRIP_MARGIN = 14


def load_font(key: str, px: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(FONTS[key][0], px)


def advance(font, text: str) -> float:
    return float(font.getlength(text))


def is_cjk(ch: str) -> bool:
    name = unicodedata.name(ch, "")
    return "CJK" in name or "HIRAGANA" in name or "KATAKANA" in name


def solo_ink(font, ch: str):
    """Rasterize one character on its own and return (bitmap bytes, ink bbox).

    This is the reference the strip is checked against; it has to be an independent
    draw, because everything the benchmark measures comes from the strip itself.
    """
    ascent, descent = font.getmetrics()
    width = int(math.ceil(max(advance(font, ch), 4.0))) + 8
    im = Image.new("L", (width, ascent + descent + 8), 255)
    ImageDraw.Draw(im).text((4, 4), ch, font=font, fill=0)
    inv = Image.eval(im, lambda v: 255 - v)
    return (inv.size, inv.tobytes()), inv.getbbox()


def ink_height_floor(ch: str, px: int) -> float:
    """How tall the drawn ink of `ch` has to be for the glyph to be believable.

    Only letters and digits are checked: punctuation is genuinely short, and a
    threshold that rejects commas would be tuned until it meant nothing.
    """
    if not ch.isalnum():
        return 0.0
    if is_cjk(ch):
        return 0.55 * px
    return 0.30 * px if ch.islower() else 0.40 * px


def check_fidelity(specs):
    """Fail before writing a single page if the rasterizer cannot draw the stimulus.

    Two independent signals, both of which were observed on this machine:
      * two different codepoints rasterize to a byte-identical bitmap -> the font has
        no glyph and substituted its box, so "ink" would describe tofu;
      * a letter's ink is far shorter than its class requires -> the glyph was
        truncated during rasterization.
    Neither is visible in the output image, and both silently hand the benchmark a
    ground-truth box around something that is not the character.
    """
    needed = {}
    for spec in specs:
        chars = needed.setdefault((spec["font"], spec["px"]), set())
        chars.update(ch for ch in spec["text"] if not ch.isspace())
    problems = []
    for (font_key, px), chars in sorted(needed.items()):
        font = load_font(font_key, px)
        signatures = {}
        for ch in sorted(chars):
            signature, box = solo_ink(font, ch)
            if box is None:
                problems.append("%s %dpx %r: nothing drawn" % (font_key, px, ch))
                continue
            signatures.setdefault(signature, []).append(ch)
            floor = ink_height_floor(ch, px)
            height = box[3] - box[1]
            if height < floor:
                problems.append(
                    "%s %dpx %r: ink %dpx tall, below the %dpx this glyph needs"
                    % (font_key, px, ch, height, math.ceil(floor))
                )
        for group in signatures.values():
            if len(group) > 1:
                problems.append(
                    "%s %dpx: %s rasterize identically (font has no such glyphs)"
                    % (font_key, px, "".join(group))
                )
    if problems:
        raise SystemExit(
            "glyph fidelity gate rejected %d font/size/char case(s):\n  %s"
            % (len(problems), "\n  ".join(problems))
        )
    return len(needed)


def render_strip(font, text: str, spacing: float):
    """Draw one horizontal line into its own strip; return strip + window helpers.

    `spacing` adds extra tracking. Any non-zero tracking forces per-character
    drawing, because tracking applied by `draw.text()` is not something we can ask
    PIL for per glyph — and a ground truth we cannot compute is not a ground truth.
    """
    if spacing == 0.0:
        pens = [advance(font, text[:i]) for i in range(len(text))]
    else:
        pens, acc = [], 0.0
        for ch in text:
            pens.append(acc)
            acc += advance(font, ch) + spacing
    total = (pens[-1] + advance(font, text[-1])) if text else 0.0
    ascent, descent = font.getmetrics()
    width = int(math.ceil(total)) + 2 * STRIP_MARGIN
    height = ascent + descent + 2 * STRIP_MARGIN
    strip = Image.new("L", (width, height), 255)
    draw = ImageDraw.Draw(strip)
    for ch, pen in zip(text, pens):
        draw.text((STRIP_MARGIN + pen, STRIP_MARGIN), ch, font=font, fill=0)

    centers = [STRIP_MARGIN + pen + advance(font, ch) / 2.0 for ch, pen in zip(text, pens)]
    bounds = []
    for i in range(len(text)):
        left = STRIP_MARGIN if i == 0 else (centers[i - 1] + centers[i]) / 2.0
        right = (
            STRIP_MARGIN + math.ceil(total)
            if i == len(text) - 1
            else (centers[i] + centers[i + 1]) / 2.0
        )
        bounds.append((left, right))
    return strip, pens, bounds, (ascent, descent), total


def ink_bbox(strip: Image.Image, box=None):
    """Bounding box of **ink** pixels (dark text on white).

    `Image.getbbox()` reports non-zero pixels, which on a white background is the
    whole image, so the measurement is taken on the inverted pixels. Getting this
    wrong silently turns every ground-truth box into its column window.
    """
    region = strip.crop(box) if box is not None else strip
    found = ImageOps.invert(region).getbbox()
    if found is None:
        return None
    if box is None:
        return found
    return (box[0] + found[0], box[1] + found[1], box[0] + found[2], box[1] + found[3])


def ink_rows(strip: Image.Image):
    box = ink_bbox(strip)
    if box is None:
        return None
    return box[1], box[3]


def char_ink_box(strip: Image.Image, x0: float, x1: float, band):
    """Ink bbox inside the column window [x0, x1) and the line's ink rows."""
    return ink_bbox(strip, (int(math.floor(x0)), band[0], int(math.ceil(x1)), band[1]))


def render_horizontal(spec):
    font = load_font(spec["font"], spec["px"])
    text = spec["text"]
    strip, pens, bounds, (ascent, descent), total = render_strip(
        font, text, spec.get("spacing", 0.0)
    )
    band = ink_rows(strip)
    if band is None:
        raise SystemExit("strip has no ink: %r" % text)
    chars = []
    for i, ch in enumerate(text):
        if ch.isspace():
            continue
        clb = font.getbbox(ch)
        ink = char_ink_box(strip, bounds[i][0], bounds[i][1], band)
        if ink is not None:
            derived = "ink"
        else:
            # Thin glyph whose ink fell entirely in a neighbour's window: fall back to
            # the metric offsets so the char is not silently dropped from GT.
            derived = "metric"
            ink = (
                STRIP_MARGIN + pens[i] + clb[0],
                STRIP_MARGIN + clb[1],
                STRIP_MARGIN + pens[i] + clb[2],
                STRIP_MARGIN + clb[3],
            )
        chars.append({"char": ch, "derived": derived, "box": [list(ink[:2]), list(ink[2:])]})
    pen_band = (
        STRIP_MARGIN,
        STRIP_MARGIN,
        STRIP_MARGIN + total,
        STRIP_MARGIN + ascent + descent,
    )
    return strip.convert("RGB"), chars, band, pen_band


def render_vertical(spec):
    """One character per row (traditional CJK column), so the same machinery applies
    with the main axis swapped."""
    font = load_font(spec["font"], spec["px"])
    text = [ch for ch in spec["text"] if not ch.isspace()]
    step = font.getmetrics()[0] + font.getmetrics()[1] + spec.get("spacing", 0.0)
    widths = [advance(font, ch) for ch in text]
    total_h = step * len(text)
    width = int(math.ceil(max(widths))) + 2 * STRIP_MARGIN
    height = int(math.ceil(total_h)) + 2 * STRIP_MARGIN
    strip = Image.new("L", (width, height), 255)
    draw = ImageDraw.Draw(strip)
    for i, (ch, w) in enumerate(zip(text, widths)):
        draw.text(
            (STRIP_MARGIN + (max(widths) - w) / 2.0, STRIP_MARGIN + i * step),
            ch,
            font=font,
            fill=0,
        )
    chars = []
    for i, ch in enumerate(text):
        center = STRIP_MARGIN + i * step + step / 2.0
        top = STRIP_MARGIN if i == 0 else (STRIP_MARGIN + (i - 1) * step + step / 2.0 + center) / 2.0
        bot = (
            STRIP_MARGIN + total_h
            if i == len(text) - 1
            else (center + STRIP_MARGIN + (i + 1) * step + step / 2.0) / 2.0
        )
        crop = ink_bbox(strip, (0, int(math.floor(top)), width, int(math.ceil(bot))))
        if crop is None:
            raise SystemExit("vertical strip row %d has no ink: %r" % (i, ch))
        # `ink_bbox` already returned strip-absolute coordinates (it adds the crop
        # offset), so re-adding `top` here would shift every vertical box down one
        # window and the ground truth would silently miss its own glyphs.
        chars.append({
            "char": ch,
            "derived": "ink",
            "box": [[crop[0], crop[1]], [crop[2], crop[3]]],
        })
    band_rows = (
        min(c["box"][0][1] for c in chars),
        max(c["box"][1][1] for c in chars),
    )
    pen_band = (STRIP_MARGIN, STRIP_MARGIN, STRIP_MARGIN + max(widths), STRIP_MARGIN + total_h)
    strip = strip.convert("RGB")
    return strip, chars, band_rows, pen_band


def as_quad(box):
    """Accept either an axis-aligned 2-corner box or an already-rotated 4-corner one."""
    if len(box) == 4:
        return [[float(p[0]), float(p[1])] for p in box]
    (x0, y0), (x1, y1) = box[0], box[1]
    return [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]


def rotate_with_map(strip: Image.Image, angle: float):
    """Rotate visually counter-clockwise and return the exact point mapping.

    PIL's semantics are asserted against a marker pixel rather than trusted: the
    whole tilted half of the benchmark depends on this mapping being right.
    """
    # `fillcolor` keeps the expanded corners white: an "L" image rotates with a
    # black fill by default, which pastes black blocks around the line and hands the
    # detector a solid rectangle to find instead of text.
    rotated = strip.rotate(angle, expand=True, resample=Image.BICUBIC, fillcolor=255)
    w, h = strip.size
    rw, rh = rotated.size
    cin = ((w - 1) / 2.0, (h - 1) / 2.0)
    cout = ((rw - 1) / 2.0, (rh - 1) / 2.0)
    theta = math.radians(angle)

    def mapping(x, y):
        dx, dy = x - cin[0], y - cin[1]
        # In image coords (y down) a visual counter-clockwise turn is this matrix.
        rx = dx * math.cos(theta) + dy * math.sin(theta)
        ry = -dx * math.sin(theta) + dy * math.cos(theta)
        return (rx + cout[0], ry + cout[1])

    probe = Image.new("L", strip.size, 0)
    px, py = (max(1, w // 4), max(1, h // 3))
    probe.putpixel((px, py), 255)
    got = probe.rotate(angle, expand=True, resample=Image.NEAREST, fillcolor=0).getbbox()
    want = mapping(px, py)
    if got is None or abs(got[0] - want[0]) > 1.5 or abs(got[1] - want[1]) > 1.5:
        raise SystemExit(
            "rotation mapping disagrees: probe landed at %s, formula says %s (angle=%s)"
            % (got, want, angle)
        )
    return rotated.convert("RGB"), mapping


def build_specs():
    """Every axis the review asked for, as an explicit list of line specs.

    Each group gets enough lines that its own metrics are readable on their own: a
    2-line group cannot tell a real regression from one bad glyph.
    """
    specs = []

    def add(group, **spec):
        spec.setdefault("spacing", 0.0)
        spec.setdefault("angle", 0.0)
        spec.setdefault("layout", "horizontal")
        spec["group"] = group
        specs.append(spec)

    def cycle(pool, i):
        return pool[i % len(pool)]

    cn_fonts = ("simsun", "simhei", "simkai", "simfang")
    for i in range(10):
        add("cn_font", font=cn_fonts[i % len(cn_fonts)], px=(32 if i < 5 else 26),
            text=cycle(CN_TEXTS, i))
    pro_fonts = ("arial", "times", "georgia", "tahoma", "segoeui")
    for i in range(10):
        add("latin_proportional", font=pro_fonts[i % len(pro_fonts)], px=(32 if i < 5 else 26),
            text=cycle(EN_TEXTS, i))
    mono_fonts = ("consolas", "lucida", "courier")
    for i in range(6):
        add("mono", font=mono_fonts[i % len(mono_fonts)], px=32, text=cycle(EN_TEXTS, i))
    digit_fonts = ("consolas", "arial", "courier", "times", "simsun", "segoeui", "tahoma", "georgia")
    for i in range(len(DIGIT_TEXTS) + 4):
        add("digits", font=digit_fonts[i % len(digit_fonts)], px=(28 if i % 2 else 32),
            text=cycle(DIGIT_TEXTS, i))
    for i, (font, px, text) in enumerate((
        ("simsun", 16, "小字号测试字符定位基准"),
        ("simhei", 14, "小字号密集排版测试样本"),
        ("arial", 16, "Small font tracking test"),
        ("times", 12, "Tiny twelve point serif"),
        ("simfang", 20, "中等字号与细字号对照"),
        ("consolas", 14, "mono-small-14pt-digits"),
        ("segoeui", 12, "12pt narrow illiterate"),
        ("simkai", 18, "小字号楷体中文样本"),
        ("tahoma", 16, "Mixed 1234 Case"),
        ("lucida", 12, "llmm 1122 small mono"),
    )):
        add("small", font=font, px=px, text=text)
    repeat_pairs = [("arial", "llllmmmm"), ("times", "WWWiii"), ("georgia", "OO00ll"),
                    ("consolas", "112233"), ("simsun", "天天密密"), ("segoeui", "nn nn mm"),
                    ("lucida", "Ill1 888 B"), ("courier", "MMWw 0O"),
                    ("simhei", "重重 叠字"), ("tahoma", "rn rn mm")]
    for font, text in repeat_pairs:
        add("repeat", font=font, px=30, text=text)
    for spacing, text, font in (
        (-2.0, "Tight tracking text", "arial"),
        (6.0, "Wide tracking text", "arial"),
        (10.0, "Very wide digits", "consolas"),
        (-1.0, "紧凑字距中文样本行", "simsun"),
        (8.0, "松排中文样本对照", "simhei"),
        (4.0, "1122 3344", "courier"),
        (-3.0, "Crowded illiterate", "times"),
        (12.0, "SPREAD CAPS", "georgia"),
    ):
        add("spacing", font=font, px=28, text=text, spacing=spacing)
    for index, angle in enumerate((-15.0, -10.0, -5.0, 5.0, 10.0, 15.0, -8.0, 12.0)):
        font = "simsun" if angle < 0 else "arial"
        pool = CN_TEXTS if angle < 0 else EN_TEXTS
        add("tilt", font=font, px=32, text=pool[index % len(pool)], angle=angle)
    for font, text in (("simsun", "竖排文本传统写法"), ("simhei", "字符定位基准"),
                       ("simkai", "楷体竖排样本行"), ("simfang", "仿宋竖排对照行")):
        add("vertical", font=font, px=30, text=text, layout="vertical")
    mixed_cn = ("simsun", "simhei", "simfang", "simkai")
    mixed_latin = ("arial", "times", "segoeui", "consolas")
    for i, text in enumerate(MIXED_TEXTS):
        # A Latin font has no glyph for these characters and substitutes its .notdef
        # box. That box carries ink, so the ground-truth machinery would happily
        # measure tofu; the script of the text picks the font pool instead.
        pool = mixed_cn if any(ord(ch) > 0x7F for ch in text) else mixed_latin
        add("mixed", font=pool[i % len(pool)], px=28, text=text)
    return specs


LINES_PER_PAGE = 8


def shifted(box, dx, dy):
    return [[p[0] + dx, p[1] + dy] for p in as_quad(box)]


def build_pages(specs, out_dir):
    pages_dir = os.path.join(out_dir, "pages")
    # A rebuild must not leave a stale page behind: the manifest would not name it,
    # but the next operator to point a viewer at the folder would see it.
    if os.path.isdir(pages_dir):
        for stale in os.listdir(pages_dir):
            if stale.endswith(".png"):
                os.remove(os.path.join(pages_dir, stale))
    os.makedirs(pages_dir, exist_ok=True)
    # Chunk inside each group rather than across the whole run. A page that mixes
    # vertical CJK columns with mixed-script lines would still score correctly (the
    # manifest tags every line), but its file name would lie and a per-page report
    # would blend two different failure modes.
    ordered = []
    for spec in specs:
        if not ordered or ordered[-1][0] != spec["group"]:
            ordered.append((spec["group"], []))
        ordered[-1][1].append(spec)
    chunks = [
        group_specs[start:start + LINES_PER_PAGE]
        for _, group_specs in ordered
        for start in range(0, len(group_specs), LINES_PER_PAGE)
    ]
    cases = []
    for group_specs in chunks:
        strips = []
        for spec in group_specs:
            if spec["layout"] == "vertical":
                strip, chars, band, pen_band = render_vertical(spec)
            else:
                strip, chars, band, pen_band = render_horizontal(spec)
            if spec["angle"] != 0.0:
                strip, mapping = rotate_with_map(strip.convert("L"), spec["angle"])

                def mapped_box(box, mapping=mapping):
                    pts = [
                        (box[0][0], box[0][1]), (box[1][0], box[0][1]),
                        (box[1][0], box[1][1]), (box[0][0], box[1][1]),
                    ]
                    out = [mapping(x, y) for x, y in pts]
                    return [[p[0], p[1]] for p in out]

                chars = [dict(c, box=mapped_box(c["box"])) for c in chars]
                pen = [mapping(pen_band[0], pen_band[1]), mapping(pen_band[2], pen_band[1]),
                       mapping(pen_band[2], pen_band[3]), mapping(pen_band[0], pen_band[3])]
            else:
                pen = [[pen_band[0], pen_band[1]], [pen_band[2], pen_band[1]],
                       [pen_band[2], pen_band[3]], [pen_band[0], pen_band[3]]]
            strips.append((spec, strip, chars, band, pen))

        height = PAGE_PAD * 2 + sum(s[1].size[1] for s in strips) + LINE_GUTTER * (len(strips) - 1)
        page = Image.new("RGB", (PAGE_WIDTH, height), "white")
        lines = []
        y = PAGE_PAD
        for spec, strip, chars, band, pen in strips:
            x = PAGE_PAD
            page.paste(strip, (x, y))
            moved = [[p[0] + x, p[1] + y] for p in pen]
            lines.append({
                "text": spec["text"],
                "group": spec["group"],
                "font": spec["font"],
                "script": FONTS[spec["font"]][1],
                "px": spec["px"],
                "angle": spec["angle"],
                "layout": spec["layout"],
                "spacing": spec["spacing"],
                "line": moved,
                "chars": [
                    dict({"char": c["char"], "box": shifted(c["box"], x, y)},
                         **({"derived": c["derived"]} if "derived" in c else {}))
                    for c in chars
                ],
            })
            y += strip.size[1] + LINE_GUTTER

        name = "pg%02d_%s.png" % (len(cases) + 1, group_specs[0]["group"])
        path = os.path.join(pages_dir, name)
        page.save(path)
        with open(path, "rb") as fh:
            digest = hashlib.sha256(fh.read()).hexdigest()
        cases.append({
            "image": "pages/" + name,
            "sha256": digest,
            "width": page.size[0],
            "height": page.size[1],
            "lines": lines,
        })
        print("%s  lines=%d  %dx%d" % (name, len(lines), page.size[0], page.size[1]))
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", default="datasets/text/charbox")
    args = parser.parse_args()
    out_dir = os.path.abspath(args.out)
    specs = build_specs()
    pairs = check_fidelity(specs)
    print("glyph fidelity gate passed: %d font/size combinations, %d characters checked"
          % (pairs, len({ch for s in specs for ch in s["text"]})))
    cases = build_pages(specs, out_dir)
    manifest = {
        "schema": 1,
        "generator": "tools/build_charbox_dataset.py",
        "case_count": len(cases),
        "line_count": sum(len(c["lines"]) for c in cases),
        "char_count": sum(len(l["chars"]) for c in cases for l in c["lines"]),
        "cases": cases,
    }
    path = os.path.join(out_dir, "manifest.json")
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(manifest, fh, ensure_ascii=False, indent=2)
    print(
        "wrote %s: %d pages / %d lines / %d chars"
        % (path, manifest["case_count"], manifest["line_count"], manifest["char_count"])
    )
    metric_only = sum(
        1 for c in cases for l in c["lines"] for ch in l["chars"] if ch.get("derived") == "metric"
    )
    print("chars whose GT fell back to font metrics (no ink in window): %d" % metric_only)
    return 0


if __name__ == "__main__":
    sys.exit(main())
