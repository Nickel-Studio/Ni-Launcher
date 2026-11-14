/**
 * 面向界面的短错误分类。
 *
 * 页面只展示一个可读的结论，详细错误仍留在调用方日志/技术详情里；新增类别时
 * 先判断是否真的能给出不同处置，避免把同类失败拆成无意义措辞。
 */
export type FailureKind =
  | "network"
  | "corrupt"
  | "argument"
  | "illegal"
  | "unknown";

const ILLEGAL =
  /非法|越权|未授权|无权限|权限不足|拒绝|不允许|禁止|key|凭证|凭据|401|403/iu;
const NETWORK =
  /网络|超时|连接|请求|接口|服务器|下载|http|dns|tls|证书|远程|镜像|timeout|connect|network|fetch|50\d/iu;
const CORRUPT =
  /损坏|校验|格式|解析|无法识别|清单|文件(已)?缺失|缺失|不存在|找不到|已变化|jar|zip|json/iu;
const ARGUMENT = /参数|无效|不合法|不支持|必须|为空|越界|超出|缺少/iu;

export function classifyFailure(reason: string): FailureKind {
  const text = reason.trim();
  if (!text) return "unknown";
  if (ILLEGAL.test(text)) return "illegal";
  if (NETWORK.test(text)) return "network";
  if (CORRUPT.test(text)) return "corrupt";
  if (ARGUMENT.test(text)) return "argument";
  return "unknown";
}

const LABELS: Record<FailureKind, string> = {
  network: "网络故障",
  corrupt: "文件损坏",
  argument: "异常参数",
  illegal: "非法调用",
  unknown: "加载失败",
};

/** 把任意错误收敛为四类短文案之一；无法归类的统一为「加载失败」。 */
export function failureLabel(reason: unknown): string {
  const text =
    typeof reason === "string"
      ? reason
      : reason instanceof Error
      ? reason.message
      : "";
  return LABELS[classifyFailure(text)];
}
