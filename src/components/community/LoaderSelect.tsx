/**
 * 检索工具栏的加载器下拉（常驻）。
 *
 * 旧的渐进式浮现（flex-basis + width + opacity 双动画）已废弃——它同时是
 * 「缩进动画抽搐」的来源；现在下拉常驻工具栏，选中版本后仅做值联动。
 * value 空串 = 「全部加载器」。
 */
import { Select, type SelectOption } from "../ui";

interface Props {
  /** 当前加载器；空串 = 「全部加载器」。 */
  value: string;
  /** 加载器选项（不含「全部加载器」，value 与后端检索参数同源）。 */
  options: ReadonlyArray<{ value: string; label: string }>;
  onChange: (value: string) => void;
}

export function LoaderSelect({ value, options, onChange }: Props) {
  const selectOptions: SelectOption[] = [
    { value: "", label: "全部加载器" },
    ...options.map((option) => ({ value: option.value, label: option.label })),
  ];

  return (
    <div className="cv-loader">
      <Select
        size="sm"
        ariaLabel="加载器"
        value={value}
        onChange={onChange}
        options={selectOptions}
        placeholder="全部加载器"
      />
    </div>
  );
}
