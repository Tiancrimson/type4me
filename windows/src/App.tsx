import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import "./App.css";

type RuntimeStatus = {
  appName: string;
  version: string;
  platform: string;
  architecture: string;
  developmentBuild: boolean;
  backendConnected: boolean;
};

type RequestState =
  | { kind: "loading" }
  | { kind: "ready"; status: RuntimeStatus }
  | { kind: "error"; message: string };

const platformLabels: Record<string, string> = {
  windows: "Windows",
  macos: "macOS",
  linux: "Linux",
};

const architectureLabels: Record<string, string> = {
  x86_64: "x64",
  aarch64: "ARM64",
};

function App() {
  const [request, setRequest] = useState<RequestState>({ kind: "loading" });

  const refreshStatus = useCallback(async () => {
    setRequest({ kind: "loading" });

    try {
      const status = await invoke<RuntimeStatus>("get_runtime_status");
      setRequest({ kind: "ready", status });
    } catch (error) {
      setRequest({
        kind: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }, []);

  useEffect(() => {
    void refreshStatus();
  }, [refreshStatus]);

  const status = request.kind === "ready" ? request.status : null;
  const isWindows = status?.platform === "windows";

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">4</span>
          <span className="brand-name">Type4Me</span>
        </div>

        <div className="sidebar-block">
          <span className="sidebar-label">平台</span>
          <strong>Windows</strong>
        </div>

        <div className="sidebar-block">
          <span className="sidebar-label">当前阶段</span>
          <strong>桌面基础</strong>
        </div>

        <div className="sidebar-note">
          <span className="status-dot" data-online={status?.backendConnected} />
          Rust 后端
        </div>
      </aside>

      <main className="workspace">
        <header className="page-header">
          <div>
            <p className="eyebrow">Type4Me / Windows</p>
            <h1>运行状态</h1>
          </div>
          <button
            className="refresh-button"
            type="button"
            onClick={() => void refreshStatus()}
            disabled={request.kind === "loading"}
          >
            {request.kind === "loading" ? "检查中" : "重新检查"}
          </button>
        </header>

        {request.kind === "error" && (
          <div className="error-banner" role="alert">
            <strong>无法连接 Rust 后端</strong>
            <span>{request.message}</span>
          </div>
        )}

        <section className="status-grid" aria-label="运行环境">
          <article className="status-card">
            <span className="card-label">操作系统</span>
            <strong>
              {status ? platformLabels[status.platform] ?? status.platform : "-"}
            </strong>
            <span className="card-detail">目标平台</span>
          </article>

          <article className="status-card">
            <span className="card-label">处理器架构</span>
            <strong>
              {status
                ? architectureLabels[status.architecture] ?? status.architecture
                : "-"}
            </strong>
            <span className="card-detail">本机运行架构</span>
          </article>

          <article className="status-card">
            <span className="card-label">应用版本</span>
            <strong>{status?.version ?? "-"}</strong>
            <span className="card-detail">
              {status ? (status.developmentBuild ? "开发构建" : "发布构建") : "读取中"}
            </span>
          </article>

          <article className="status-card">
            <span className="card-label">Tauri 通道</span>
            <strong>{status?.backendConnected ? "已连接" : "未连接"}</strong>
            <span className="card-detail">前端调用 Rust 命令</span>
          </article>
        </section>

        <section className="system-state">
          <div>
            <p className="eyebrow">基础检查</p>
            <h2>{isWindows ? "Windows 环境已就绪" : "等待 Windows 环境"}</h2>
          </div>
          <div className="state-marker" data-ready={isWindows}>
            <span />
            {isWindows ? "可继续开发" : "平台不匹配"}
          </div>
        </section>
      </main>
    </div>
  );
}

export default App;
