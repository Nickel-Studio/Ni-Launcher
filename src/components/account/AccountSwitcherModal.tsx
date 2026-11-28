import { useMemo } from "react";
import { Button, Modal } from "../ui";
import { CharacterAvatar } from "./CharacterAvatar";
import { accountSubtitle, accountTypeLabel, type Account } from "./types";
import "./account.css";

export interface AccountSwitcherModalProps {
  open: boolean;
  onClose: () => void;
  accounts: Account[];
  activeId: string;
  onSelect: (id: string) => void;
  onRemove: (id: string) => void;
  /** 保留当前账号，进入可返回的临时登录表单。 */
  onLoginOther: () => void;
}

/**
 * 「切换账号」弹窗：列出已保存的账号，点一行即切换（必要时连同账号形态一起切），
 * 也可删除单个账号。「登录其他账号」走真实的登录表单，不在这里凭空造一条没有令牌的记录。
 */
export function AccountSwitcherModal({
  open,
  onClose,
  accounts,
  activeId,
  onSelect,
  onRemove,
  onLoginOther,
}: AccountSwitcherModalProps) {
  const sorted = useMemo(
    () => [...accounts].sort((a, b) => a.type.localeCompare(b.type)),
    [accounts]
  );

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="切换账号"
      width={480}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            关闭
          </Button>
          <Button variant="primary" onClick={onLoginOther}>
            登录其他账号
          </Button>
        </>
      }
    >
      <div className="acc-switch">
        <div className="acc-switch__list">
          {sorted.length === 0 ? (
            <p className="acc-switch__empty">还没有已保存的账号。</p>
          ) : (
            sorted.map((account) => {
              const active = account.id === activeId;
              return (
                <div
                  className="acc-switch__row"
                  key={account.id}
                  data-active={active}
                >
                  <button
                    type="button"
                    className="acc-switch__pick"
                    onClick={() => {
                      onSelect(account.id);
                      onClose();
                    }}
                    aria-current={active}
                  >
                    <CharacterAvatar
                      name={account.name}
                      size={34}
                      source={account.type}
                      uuid={account.uuid}
                      server={account.server}
                    />
                    <span className="acc-switch__meta">
                      <span className="acc-switch__name">
                        {account.name || "未命名"}
                      </span>
                      <span className="acc-switch__sub">
                        <span className="acc-switch__type">
                          {accountTypeLabel(account.type)}
                        </span>
                        {accountSubtitle(account)}
                      </span>
                    </span>
                    {active ? (
                      <span className="acc-switch__check">当前</span>
                    ) : null}
                  </button>
                  <Button
                    variant="ghost"
                    size="sm"
                    icon="trash"
                    aria-label={`删除 ${account.name}`}
                    onClick={() => onRemove(account.id)}
                  >
                    删除
                  </Button>
                </div>
              );
            })
          )}
        </div>
      </div>
    </Modal>
  );
}

export default AccountSwitcherModal;
