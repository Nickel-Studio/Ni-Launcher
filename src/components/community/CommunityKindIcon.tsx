/**
 * 社区分类的统一图标映射。
 *
 * 侧边栏、结果卡兜底与详情页共享同一组资源 SVG，避免不同页面各自维护
 * 图标或退化成标题首字母。
 */
import type { ComponentType } from "react";
import type { ModrinthKind } from "../modrinth/types";
import type { IconProps } from "../ui/icons";
import {
  IconBundle,
  IconMods,
  IconPack,
  IconShader,
  IconWorld,
} from "../resources/icons";

const KIND_ICONS: Record<ModrinthKind, ComponentType<IconProps>> = {
  mod: IconMods,
  modpack: IconBundle,
  datapack: IconWorld,
  resourcepack: IconPack,
  shader: IconShader,
};

interface Props extends IconProps {
  kind: ModrinthKind;
}

export function CommunityKindIcon({ kind, ...props }: Props) {
  const Icon = KIND_ICONS[kind];
  return <Icon {...props} />;
}
