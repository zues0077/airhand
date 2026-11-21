// 软件内的中转设置：地址、证书指纹都在这里改，不用再打开安装目录里的 config.toml。
// 只在桌面版显示（网页版没有这个入口，也没有 set_relay 命令）。
import { useEffect, useRef, useState } from "react";
import { api, type Problem } from "../lib/api";
import { useT } from "../lib/i18n";
import { problemText } from "../lib/messages";
import { store, useStore } from "../lib/store";

/** 两端的预设保持一致（安卓端见 RelayPresets.kt） */
const PRESETS = [
  {
    key: "self",
    server: "airhand.fnosliu.de5.net:52141",
    fp: "a27582a2522417fe6058e545592ec289fedcf67a0575909783441c897ecb2abf",
  },
  {
    key: "public",
    server: "39.105.40.56:4433",
    fp: "5c1e5ad85089bfa938b8b7f5008076c42ad6dfe3aee5e623300ef93d0efa8fbe",
  },
] as const;

function validServer(v: string): boolean {
  const s = v.trim();
  const i = s.lastIndexOf(":");
  if (i <= 0) return false;
  const host = s.slice(0, i);
  const port = Number(s.slice(i + 1));
  if (!Number.isInteger(port) || port < 1 || port > 65535) return false;
  return /^[a-zA-Z0-9._-]+$/.test(host);
}

function validFp(v: string): boolean {
  const s = v.trim();
  if (!s) return true; // 留空 = 第一次连接时记下
  const hex = s.replace(/[\s:]/g, "");
  return /^[0-9a-fA-F]{64}$/.test(hex);
}

export function SettingsDialog() {
  const t = useT();
  const open = useStore((s) => s.relayDialog);
  const app = useStore((s) => s.app);
  const [server, setServer] = useState("");
  const [fp, setFp] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [done, setDone] = useState(false);
  // 切换成功后关闭控制台窗口的定时器（用户点"取消"时取消它，别把人正在看的窗口关掉）
  const closeTimer = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (closeTimer.current !== null) clearTimeout(closeTimer.current);
    },
    [],
  );

  // 每次打开都用当前生效值回填
  useEffect(() => {
    if (!open) return;
    setServer(app?.server ?? "");
    setFp(app?.serverFp ?? "");
    setError("");
    setDone(false);
  }, [open, app?.server, app?.serverFp]);

  if (!open) return null;
  const close = () => {
    if (closeTimer.current !== null) {
      clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
    store.setRelayDialog(false);
  };

  const apply = async (s: string, f: string) => {
    const server = s.trim();
    const fp = f.trim();
    if (!validServer(server)) {
      setError(t("relay.invalidServer"));
      return;
    }
    if (!validFp(fp)) {
      setError(t("relay.invalidFp"));
      return;
    }
    setBusy(true);
    setError("");
    setDone(false);
    try {
      await api.setRelay(server, fp);
      // 服务写文件会有一下延迟；刷新 appInfo 回填真正生效的值
      const fresh = await api.appInfo();
      store.setApp(fresh);
      setServer(fresh.server);
      setFp(fresh.serverFp ?? "");
      setDone(true);
      // 切换中转后服务端会结束当前控制台会话（见 console.rs 里监听 relay_epoch 的那段）：
      // 画面停在最后一帧、点哪都没反应，看起来像卡死。桌面版主动关掉控制台窗口（回到被控端模式），
      // 用户重新打开控制台时就会用新中转连接；网页版没有窗口可关，提示刷新页面。
      const frame = api.frame;
      if (frame) closeTimer.current = window.setTimeout(() => void frame.destroy(), 1500);
    } catch (e) {
      setError(problemText(e as Problem));
    } finally {
      setBusy(false);
    }
  };

  const same = server.trim() === app?.server && fp.trim() === (app?.serverFp ?? "");

  return (
    <div
      className="modal-backdrop"
      data-tauri-drag-region="false"
      onPointerDown={(e) => e.target === e.currentTarget && close()}
    >
      <div className="modal relay-dialog" role="dialog" aria-modal="true" aria-labelledby="relay-title">
        <h3 id="relay-title">{t("relay.title")}</h3>

        <div className="relay-current">
          <span className="am-label">{t("relay.current")}</span>
          <span className="mono">{app?.server || "—"}</span>
        </div>

        <div className="relay-presets">
          <span className="am-label">{t("relay.presets")}</span>
          <div className="relay-preset-row">
            {PRESETS.map((p) => (
              <button
                key={p.key}
                className="ghost-btn"
                disabled={busy}
                onClick={() => {
                  setServer(p.server);
                  setFp(p.fp);
                  setError("");
                  setDone(false);
                }}
              >
                {t(`relay.preset.${p.key}`)}
              </button>
            ))}
          </div>
        </div>

        <label className="relay-field">
          <span className="am-label">{t("server.address")}</span>
          <input
            className="relay-input mono"
            value={server}
            onChange={(e) => {
              setServer(e.target.value);
              setDone(false);
            }}
            placeholder="host:port"
            spellCheck={false}
          />
          <span className="muted small">{t("relay.serverHint")}</span>
        </label>

        <label className="relay-field">
          <span className="am-label">{t("relay.fp")}</span>
          <input
            className="relay-input mono"
            value={fp}
            onChange={(e) => {
              setFp(e.target.value);
              setDone(false);
            }}
            placeholder={t("relay.fpPlaceholder")}
            spellCheck={false}
          />
          <span className="muted small">{t("relay.fpHint")}</span>
        </label>

        <p className="relay-warn">{t("relay.notice")}</p>
        {error && <p className="relay-error">{error}</p>}
        {done && (
          <p className="relay-done">{t(api.frame ? "relay.doneClosing" : "relay.doneRefresh")}</p>
        )}

        <div className="modal-actions">
          <button className="ghost-btn" onClick={close}>
            {t("confirm.cancel")}
          </button>
          <button
            className="danger-btn"
            disabled={busy || same}
            onClick={() => void apply(server, fp)}
          >
            {busy ? t("relay.applying") : t("relay.apply")}
          </button>
        </div>
      </div>
    </div>
  );
}
