#!/usr/bin/env bash
# 为桌面 GUI 准备构建所需的原生资源：中文字体。
#
# 字体与 libmpv 一样是原生依赖，不随仓库分发（见 docs/GUI.md）。
# 已经装过 Noto Sans CJK 的系统直接软链接；否则从 Google Fonts 下载 OFL 授权版本。
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
target="$root/gui/assets/NotoSansCJK-Regular.ttc"
mkdir -p "$(dirname "$target")"

if [[ -f "$target" ]]; then
  echo "字体已存在：$target"
  exit 0
fi

for candidate in \
  /usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc \
  /usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc \
  /usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc \
  /Library/Fonts/NotoSansCJK-Regular.ttc; do
  if [[ -f "$candidate" ]]; then
    echo "使用系统字体：$candidate"
    ln -sf "$candidate" "$target"
    exit 0
  fi
done

# Google Fonts 的 NotoSansSC 可变字重字体（SIL OFL 1.1）；freetype 按内容
# 识别格式，所以仍沿用 .ttc 这个文件名。
url="https://github.com/google/fonts/raw/main/ofl/notosanssc/NotoSansSC%5Bwght%5D.ttf"
echo "从 $url 下载字体"
curl --location --fail --retry 3 --output "$target" "$url"
echo "已保存：$target"
