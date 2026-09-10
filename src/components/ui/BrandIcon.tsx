import type { ImgHTMLAttributes } from "react";
import { cn } from "@/lib/utils";

/**
 * 品牌图标（P7）：各环境卡使用工具官网/官方仓库原始图标（非 lucide 通用图标）。
 * 资源来源与许可（本地化存放于 src/assets/brands/，避免运行时外链）：
 * - python.png   python.org favicon
 * - node.png     nodejs.org favicon
 * - frida.png    frida.re favicon
 * - ida.png      hex-rays.com 官网 ladyida 官方形象素材（站内 `ladyida.png`）
 * - jadx.png     github.com/skylot/jadx（official repo logo）
 * - android.png  Android 官方机器人（developer.android.com 品牌素材 / robots 矢量）
 * 仅作「识别用途」展示各工具品牌；商标归各自所有者。
 */

import androidIcon from "@/assets/brands/android.png";
import fridaIcon from "@/assets/brands/frida.png";
import idaIcon from "@/assets/brands/ida.png";
import jadxIcon from "@/assets/brands/jadx.png";
import nodeIcon from "@/assets/brands/node.png";
import pythonIcon from "@/assets/brands/python.png";

export const BRAND_ICONS = {
  python: pythonIcon,
  node: nodeIcon,
  frida: fridaIcon,
  ida: idaIcon,
  jadx: jadxIcon,
  android: androidIcon,
} as const;

export type BrandName = keyof typeof BRAND_ICONS;

export function BrandIcon({
  name,
  className,
  ...rest
}: { name: BrandName } & ImgHTMLAttributes<HTMLImageElement>) {
  return (
    <img
      src={BRAND_ICONS[name]}
      alt=""
      aria-hidden="true"
      draggable={false}
      className={cn("h-4 w-4 shrink-0 select-none object-contain", className)}
      {...rest}
    />
  );
}
