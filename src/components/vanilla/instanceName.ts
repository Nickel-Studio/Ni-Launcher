/** 与后端 named_profiles::validate_name 保持一致；后端再次校验并拒绝重名覆盖。 */
export function instanceNameError(name: string): string {
  if (!name || name.trim() !== name) return "请输入实例名称，且不要以空格开头或结尾。";
  if (new TextEncoder().encode(name).length > 160) return "实例名称过长，请缩短到 160 字节以内。";
  if (name.includes("..") || name.endsWith(".") || /[<>:"/\\|?*\u0000-\u001f\u007f-\u009f]/.test(name)) {
    return "名称不能包含路径分隔符、..、尾随句点或 <>:\"|?*。";
  }
  if (/^(con|prn|aux|nul|clock\$|conin\$|conout\$|com[1-9¹²³]|lpt[1-9¹²³])$/i.test(name.split(".")[0].trimEnd())) {
    return "这是系统保留的文件名，请换一个名称。";
  }
  if (["nickel.profile", "gamedata", "nickel.settings"].includes(name.toLowerCase())) {
    return "名称与实例数据文件冲突，请换一个名称。";
  }
  return "";
}
