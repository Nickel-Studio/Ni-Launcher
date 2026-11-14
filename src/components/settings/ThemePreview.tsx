import { PRODUCT_NAME } from "../../lib/brand";

export function ThemePreview() {
  return (
    <div className="st-preview" role="img" aria-label="当前外观实时预览">
      <div className="st-preview__bar">
        <span className="st-preview__dot" />
        <span className="st-preview__dot" />
        <span className="st-preview__dot" />
        <span className="st-preview__bar-title">{PRODUCT_NAME} · 预览</span>
      </div>
      <div className="st-preview__body">
        <div className="st-preview__col">
          <span className="st-preview__eyebrow">实时预览</span>
          <h4 className="st-preview__h">生存世界 · Survival</h4>
          <p className="st-preview__p">1.21.1 · Fabric · 已就绪</p>
          <div className="st-preview__chips">
            <span className="st-preview__chip">圆角</span>
            <span className="st-preview__chip">表面色</span>
            <span className="st-preview__chip st-preview__chip--accent">
              颜色主题
            </span>
          </div>
        </div>
        <div className="st-preview__col st-preview__col--right">
          <div className="st-preview__btnrow">
            <span className="st-preview__btn st-preview__btn--ghost">设置</span>
            <span className="st-preview__btn st-preview__btn--accent">
              启动游戏
            </span>
          </div>
          <div className="st-preview__progress">
            <span className="st-preview__progress-fill" />
          </div>
          <div className="st-preview__toggle">
            <span className="st-preview__toggle-knob" />
          </div>
        </div>
      </div>
    </div>
  );
}
