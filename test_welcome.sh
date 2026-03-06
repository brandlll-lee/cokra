#!/bin/bash

# Cokra Welcome界面测试脚本
# 用于验证重构后的左对齐布局和纯白色logo

set -e

echo "================================"
echo "Cokra Welcome界面重构测试"
echo "================================"
echo ""

# 设置颜色
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# 项目根目录
COKRA_DIR="/mnt/f/CodeHub/leehub/cokra/cokra-rs"
BINARY="$COKRA_DIR/target-local/debug/cokra"

echo "检查可执行文件..."
if [ ! -f "$BINARY" ]; then
    echo -e "${YELLOW}警告: 未找到已构建的二进制文件${NC}"
    echo "正在构建..."
    cd "$COKRA_DIR"
    cargo build --package cokra
    BINARY="$COKRA_DIR/target/debug/cokra"
fi

if [ ! -f "$BINARY" ]; then
    echo "❌ 构建失败：未找到cokra可执行文件"
    exit 1
fi

echo -e "${GREEN}✅ 找到可执行文件: $BINARY${NC}"
echo ""

echo "测试命令："
echo "$BINARY --ui-mode inline -c models.provider=openrouter -c models.model=openrouter/anthropic/claude-haiku-4.5"
echo ""

echo "================================"
echo "预期效果验证清单："
echo "================================"
echo "1. ☐ Logo 左对齐显示（不是居中）"
echo "2. ☐ Logo 纯白色极简风格（不是薄荷蓝）"
echo "3. ☐ 欢迎文本左对齐"
echo "4. ☐ 所有元素有2个空格的左缩进"
echo "5. ☐ 与codex布局方式一致"
echo ""

echo "按Enter键启动cokra inline模式..."
read

echo "启动cokra..."
echo ""

# 运行cokra
cd "$COKRA_DIR"
"$BINARY" \
  --ui-mode inline \
  -c models.provider=openrouter \
  -c models.model=openrouter/anthropic/claude-haiku-4.5

echo ""
echo "================================"
echo "测试完成"
echo "================================"
echo "请验证上述5个检查项是否都通过 ✅"
