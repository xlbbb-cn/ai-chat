#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Tauri 全平台图标一键生成脚本（Windows / macOS / Linux 通用）。

只依赖 Pillow：不需要 ImageMagick，也不需要 macOS 的 sips / iconutil。
icon.icns 由脚本按 ICNS 容器格式直接封装（PNG 压缩块，macOS 10.7+ 支持），
因此在 Windows 上也能生成 macOS 图标，一次运行即可产出三平台所需的全部素材。

用法示例：
    python generate_ico.py                          # 自动挑源图，生成全部图标
    python generate_ico.py --source logo.png        # 指定源图
    python generate_ico.py --outdir build/icons     # 输出到其它目录（默认脚本所在目录）
    python generate_ico.py --no-round               # 保留直角（不裁圆角）
    python generate_ico.py --trim --pad 0.08        # 先裁掉四周空白，再统一留 8% 透明边距
    python generate_ico.py --sharpen                # 小尺寸下采样后轻微锐化
    python generate_ico.py --skip-web               # 不生成 public/favicon.*

生成产物：
    <src-tauri/icons>/  32x32.png  128x128.png  128x128@2x.png  icon.png (1024)
                        icon.ico    icon.icns   Square*Logo.png  StoreLogo.png
    <repo>/public/      favicon.png favicon.ico

源图按以下顺序自动查找（第一个存在的即被采用）：
    logo.png → icon.png → rounded_image.png → XLBBB-LOGO.png

提示：macOS 上若 Finder / Dock 仍显示旧图标，执行
    rm -rf ~/Library/Caches/com.apple.iconservices.store && killall Dock
"""

from __future__ import annotations

import argparse
import io
import struct
import sys
from pathlib import Path

try:
    from PIL import Image, ImageChops, ImageDraw, ImageFilter
except ImportError:  # pragma: no cover
    sys.exit("缺少 Pillow，请先安装：  python -m pip install pillow")

# --------------------------------------------------------------------- 常量

SCRIPT_DIR = Path(__file__).resolve().parent      # src-tauri/icons
REPO_ROOT = SCRIPT_DIR.parent.parent              # 仓库根目录
PUBLIC_DIR = REPO_ROOT / "public"

SOURCE_CANDIDATES = ("logo.png", "icon.png", "rounded_image.png", "XLBBB-LOGO.png")

MASTER_SIZE = 1024          # 统一母版尺寸
DEFAULT_RADIUS = 0.20       # 圆角半径占边长的比例（0 = 直角）

# 主图标 PNG：文件名 -> 像素尺寸
PNG_TARGETS = {
    "32x32.png": 32,
    "128x128.png": 128,
    "128x128@2x.png": 256,
    "icon.png": MASTER_SIZE,
}

ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)

# ICNS 类型码 -> 尺寸；全部使用 PNG 压缩块。
# icp4~icp6：16/32/64 的 1x 变体；ic07~ic10：128/256/512/1024；
# ic11~ic14：16/32/128/256 的 @2x 变体（与上面尺寸重复但类型码不同，属正常）。
ICNS_TARGETS = (
    ("icp4", 16),
    ("icp5", 32),
    ("icp6", 64),
    ("ic07", 128),
    ("ic08", 256),
    ("ic09", 512),
    ("ic10", 1024),
    ("ic11", 32),
    ("ic12", 64),
    ("ic13", 256),
    ("ic14", 512),
)

# Windows 应用商店图标（与 `tauri icon` 生成的文件名保持一致）
STORE_TARGETS = {
    "Square30x30Logo.png": 30,
    "Square44x44Logo.png": 44,
    "Square71x71Logo.png": 71,
    "Square89x89Logo.png": 89,
    "Square107x107Logo.png": 107,
    "Square142x142Logo.png": 142,
    "Square150x150Logo.png": 150,
    "Square284x284Logo.png": 284,
    "Square310x310Logo.png": 310,
    "StoreLogo.png": 50,
}

FAVICON_PNG_SIZE = 256
FAVICON_ICO_SIZES = (16, 32, 48)

RESAMPLE = getattr(Image, "Resampling", Image).LANCZOS


# ----------------------------------------------------------------- 图像处理

def pick_source(explicit: Path | None) -> Path:
    """返回源图路径：显式指定优先，否则按候选列表查找。"""
    if explicit is not None:
        if not explicit.is_file():
            sys.exit(f"源图不存在：{explicit}")
        return explicit
    for name in SOURCE_CANDIDATES:
        candidate = SCRIPT_DIR / name
        if candidate.is_file():
            return candidate
    sys.exit("未找到源图，请用 --source 指定。候选：" + "、".join(SOURCE_CANDIDATES))


def content_bbox(img: Image.Image) -> tuple[int, int, int, int] | None:
    """内容边界：优先用 alpha 通道；整图不透明时退化为“与左上角背景色不同”的区域。"""
    alpha = img.getchannel("A")
    if alpha.getextrema()[0] < 255:
        return alpha.point(lambda v: 255 if v > 0 else 0).getbbox()
    background = img.getpixel((0, 0))[:3]
    diff = ImageChops.difference(img.convert("RGB"), Image.new("RGB", img.size, background))
    return diff.convert("L").point(lambda v: 255 if v > 16 else 0).getbbox()


def round_corners(img: Image.Image, radius: float) -> Image.Image:
    """套圆角遮罩；与已有 alpha 相乘，原本透明的区域保持透明。"""
    radius = max(0.0, min(0.5, radius))
    if radius <= 0:
        return img
    w, h = img.size
    scale = 4  # 4 倍超采样，避免圆角出现锯齿
    mask = Image.new("L", (w * scale, h * scale), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, w * scale - 1, h * scale - 1),
        radius=radius * min(w, h) * scale,
        fill=255,
    )
    mask = mask.resize((w, h), RESAMPLE)
    out = img.copy()
    out.putalpha(ImageChops.multiply(img.getchannel("A"), mask))
    return out


def add_padding(img: Image.Image, fraction: float) -> Image.Image:
    """四周加透明边距，fraction 为边长比例（0.08 = 每边 8%）。"""
    if fraction <= 0:
        return img
    w, h = img.size
    inner = max(1, int(round(w / (1 + 2 * fraction))))
    scaled = img.resize((inner, inner), RESAMPLE)
    canvas = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    offset = (w - inner) // 2
    canvas.paste(scaled, (offset, offset), scaled)
    return canvas


def render(master: Image.Image, size: int, sharpen: bool = False) -> Image.Image:
    """从母版渲染指定尺寸；锐化只作用于 RGB，避免透明边缘产生亮边。"""
    img = master if size == master.width else master.resize((size, size), RESAMPLE)
    img = img.copy()
    if sharpen:
        sharpened = img.convert("RGB").filter(
            ImageFilter.UnsharpMask(radius=0.6, percent=80, threshold=2)
        )
        img = Image.merge("RGBA", (*sharpened.split(), img.getchannel("A")))
    return img


def load_master(path: Path, *, trim: bool, radius: float, pad: float) -> Image.Image:
    """读取源图并归一化为正方形 RGBA 母版（可选裁剪 / 圆角 / 边距）。"""
    with Image.open(path) as raw:
        img = raw.convert("RGBA")  # convert 会强制加载像素，之后关闭文件句柄是安全的

    if trim:
        bbox = content_bbox(img)
        if bbox:
            img = img.crop(bbox)

    side = min(img.size)
    left = (img.width - side) // 2
    top = (img.height - side) // 2
    img = img.crop((left, top, left + side, top + side))   # 居中裁成正方形
    img = img.resize((MASTER_SIZE, MASTER_SIZE), RESAMPLE)

    if radius > 0:
        img = round_corners(img, radius)
    return add_padding(img, pad)


# --------------------------------------------------------------------- 导出

def png_bytes(img: Image.Image) -> bytes:
    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def save_ico(master: Image.Image, path: Path, sizes=ICO_SIZES, sharpen: bool = False) -> None:
    """多分辨率 .ico（Pillow 内部按 LANCZOS 逐级缩放）。"""
    render(master, max(sizes), sharpen).save(
        path, format="ICO", sizes=[(s, s) for s in sizes]
    )


def save_icns(master: Image.Image, path: Path) -> None:
    """纯 Python 写 ICNS：'icns' + 总长度 + 若干个 (类型码 + 长度 + PNG) 块。"""
    blocks = []
    for code, size in ICNS_TARGETS:
        data = png_bytes(render(master, size))
        blocks.append(code.encode("ascii") + struct.pack(">I", 8 + len(data)) + data)
    body = b"".join(blocks)
    path.write_bytes(b"icns" + struct.pack(">I", 8 + len(body)) + body)


def display(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


# --------------------------------------------------------------------- 主流程

def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Tauri 图标一键生成（PNG / ICO / ICNS / 应用商店图 / favicon）",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument("--source", type=Path, help="源图路径（建议 1024x1024 正方形）",default="src-tauri/icons/XLBBB-LOGO.png")
    parser.add_argument("--outdir", type=Path, default=SCRIPT_DIR, help="图标输出目录")
    parser.add_argument("--radius", type=float, default=DEFAULT_RADIUS, help="圆角半径比例，0 表示直角")
    parser.add_argument("--no-round", action="store_true", help="等价于 --radius 0")
    parser.add_argument("--trim", action="store_true", help="生成前先裁掉四周空白 / 透明边")
    parser.add_argument("--pad", type=float, default=0.0, help="四周额外透明边距比例")
    parser.add_argument("--sharpen", action="store_true", help="小尺寸下采样后轻微锐化")
    parser.add_argument("--skip-web", action="store_true", help="不生成 public/favicon.*")
    args = parser.parse_args(argv)

    source = pick_source(args.source)
    radius = 0.0 if args.no_round else args.radius
    master = load_master(source, trim=args.trim, radius=radius, pad=args.pad)

    outdir: Path = args.outdir
    outdir.mkdir(parents=True, exist_ok=True)

    print(f"源图：{display(source)}  {source.stat().st_size / 1024:.1f} KB")
    print(
        f"母版：{MASTER_SIZE}x{MASTER_SIZE} RGBA   圆角 {radius:.0%}   "
        f"裁剪 {'开' if args.trim else '关'}   边距 {args.pad:.0%}   锐化 {'开' if args.sharpen else '关'}"
    )

    written: list[Path] = []

    for name, size in {**PNG_TARGETS, **STORE_TARGETS}.items():
        target = outdir / name
        img = render(master, size, sharpen=args.sharpen and size <= 64)
        img.save(target, format="PNG", optimize=True)
        written.append(target)

    ico = outdir / "icon.ico"
    save_ico(master, ico, sharpen=args.sharpen)
    written.append(ico)

    icns = outdir / "icon.icns"
    save_icns(master, icns)
    written.append(icns)

    if not args.skip_web:
        if PUBLIC_DIR.is_dir():
            favicon_png = PUBLIC_DIR / "favicon.png"
            favicon_ico = PUBLIC_DIR / "favicon.ico"
            render(master, FAVICON_PNG_SIZE).save(favicon_png, format="PNG", optimize=True)
            save_ico(master, favicon_ico, FAVICON_ICO_SIZES)
            written += [favicon_png, favicon_ico]
        else:
            print(f"跳过 favicon：目录不存在 {PUBLIC_DIR}")

    for path in written:
        print(f"  ✔ {display(path):<42} {path.stat().st_size / 1024:>8.1f} KB")
    print(f"完成：共生成 {len(written)} 个文件")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())