//! tie 语言支持扩展 —— VSCode 客户端（TypeScript + vscode-languageclient 标准实现）。
//!
//! 职责：
//! - 按配置 tie.lsp.command 启动 tie 语言服务器（默认 `tie --lsp`，支持绝对路径）；
//! - 经 vscode-languageclient 自动完成协议对接（无需手工分帧）：
//!   - 文档同步：didOpen / didChange / didClose（服务器 textDocumentSync=1 全量，自动协商）；
//!   - 诊断：publishDiagnostics 推送 → 显示在「问题」面板；
//!   - hover：悬停显示函数签名等；
//!   - 跳转定义（textDocument/definition）与补全（textDocument/completion，
//!     triggerCharacters 含 "."）——后端正在新增，均为标准 LSP 方法，
//!     本客户端无需特判，库自动处理；
//! - 服务器不在 PATH / 启动失败时，给出可操作的错误提示。

import * as vscode from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    ErrorHandler,
    ErrorAction,
    CloseAction,
    RevealOutputChannelOn,
    State,
} from 'vscode-languageclient/node';

/** 配置节（与 package.json contributes.configuration 的 title 对应） */
const CONFIG_SECTION = 'tie';
/** 配置项：语言服务器启动命令（数组：[命令, 参数...]） */
const CONFIG_COMMAND = 'lsp.command';
/** 默认命令：tie 工具链在 PATH 中时直接可用 */
const DEFAULT_COMMAND: string[] = ['tie', '--lsp'];
/** 服务器名称：用作输出通道名与 LanguageClient 标识 */
const SERVER_NAME = 'tie-lsp';
/** 输出通道：显示服务器 stderr 与客户端日志 */
const OUTPUT_CHANNEL_NAME = 'tie';

/** 当前语言客户端实例（deactivate 时停止） */
let client: LanguageClient | undefined;
/** 输出通道实例 */
let outputChannel: vscode.OutputChannel;
/** 服务器是否成功完成过握手（用于区分「启动失败」与「运行中退出」） */
let serverReady = false;
/** 是否已弹过启动失败提示（避免反复弹窗打扰） */
let startupErrorShown = false;

/**
 * 自定义错误处理：服务器不在 PATH（spawn ENOENT）或启动崩溃时给出可操作提示。
 *
 * vscode-languageclient 在子进程启动失败时会依次回调 error() 与 closed()；
 * 由于服务器从未成功握手（serverReady=false），此时提示用户检查安装与配置。
 */
class TieErrorHandler implements ErrorHandler {
    /** 最近一次错误信息（用于提示详情） */
    private lastError = '';

    error(error: Error, _message: unknown, _count: number | undefined) {
        this.lastError = error?.message ?? '';
        // 启动期错误 → 直接关停，交给 closed() 统一提示
        return { action: ErrorAction.Shutdown } as const;
    }

    closed() {
        // 服务器从未成功握手 → 大概率是命令不在 PATH 或路径配置错误
        if (!serverReady && !startupErrorShown) {
            startupErrorShown = true;
            const detail = this.lastError ? `：${this.lastError}` : '';
            void vscode.window.showErrorMessage(
                `tie 语言服务器启动失败${detail}。请确认：` +
                '① 已安装 tie 工具链，且终端中可运行 `tie --lsp`；' +
                '② 或在工作区设置中把 tie.lsp.command 配置为服务器绝对路径' +
                '（如 ["F:/Projects/tie/target/debug/tie.exe", "--lsp"]）。' +
                '详见扩展 README 的「配置」一节。'
            );
        }
        // 不自动重启（避免崩溃-重启死循环），用户可执行「重新加载窗口」恢复
        return { action: CloseAction.DoNotRestart, handled: true } as const;
    }
}

/**
 * 从工作区配置读取语言服务器命令。
 *
 * 支持两种形态：
 * - 默认 ['tie', '--lsp']：tie 工具链在 PATH 中；
 * - 绝对路径 ['F:/.../tie.exe', '--lsp']：工具链不在 PATH 时。
 * 配置非法时回退默认值并输出警告。
 */
function readServerCommand(): string[] {
    const cfg = vscode.workspace.getConfiguration(CONFIG_SECTION);
    const cmd = cfg.get<string[]>(CONFIG_COMMAND, DEFAULT_COMMAND);
    if (!Array.isArray(cmd) || cmd.length === 0 || typeof cmd[0] !== 'string') {
        outputChannel.appendLine(
            `警告：tie.lsp.command 配置无效（${JSON.stringify(cmd)}），已回退默认值 ${JSON.stringify(DEFAULT_COMMAND)}`
        );
        return [...DEFAULT_COMMAND];
    }
    return cmd.map((c) => String(c));
}

/** 激活扩展：由 activationEvents onLanguage:tie 触发，创建并启动语言客户端。 */
export function activate(context: vscode.ExtensionContext): void {
    // 输出通道：服务器 stderr 与客户端日志（「输出 → tie」）
    outputChannel = vscode.window.createOutputChannel(OUTPUT_CHANNEL_NAME);

    // 服务器命令：从 tie.lsp.command 配置读取（默认 ['tie', '--lsp']）
    const [command, ...args] = readServerCommand();
    outputChannel.appendLine(`启动语言服务器：${command} ${args.join(' ')}`);

    // 服务器选项：spawn 子进程（stdio 管道，LSP over stdio）
    const serverOptions: ServerOptions = {
        command,
        args,
    };

    // 客户端选项：文档选择 / 输出 / 错误处理
    const clientOptions: LanguageClientOptions = {
        // 只处理 tie 语言文档（语言 ID 由 package.json 的 languages 贡献注册）
        documentSelector: [{ language: 'tie' }],
        // 输出通道：服务器 stderr 与日志均进「输出 → tie」
        outputChannel,
        // 服务器主动报错时弹出输出通道（日常静默，不打扰）
        revealOutputChannelOn: RevealOutputChannelOn.Error,
        // 自定义错误处理：启动失败给可操作提示
        errorHandler: new TieErrorHandler(),
        // 连接稳定后标记 serverReady（供错误处理区分启动失败）
        initializationFailedHandler: () => {
            // initialize 失败（如协议不兼容）——同样给出提示后不再重试
            if (!startupErrorShown) {
                startupErrorShown = true;
                void vscode.window.showErrorMessage(
                    'tie 语言服务器初始化失败：请确认 tie --lsp 版本与扩展兼容，查看「输出 → tie」了解详情。'
                );
            }
            return false; // 不自动重启
        },
    };

    // 客户端实例：名称 tie-lsp（输出通道、错误提示均用此名）
    client = new LanguageClient(SERVER_NAME, 'tie 语言服务器', serverOptions, clientOptions);

    // 监听客户端进入运行态（initialize 握手完成）→ 标记 serverReady
    // （供错误处理区分「启动失败」与「运行中退出」）
    client.onDidChangeState((e) => {
        if (e.newState === State.Running) {
            serverReady = true;
        }
    });

    // 启动客户端（8.1.0 起 start() 返回 Promise<void>，不阻塞 activate；
    // 启动失败会走 errorHandler / initializationFailedHandler 提示）
    void client.start().catch((err: Error) => {
        outputChannel.appendLine(`语言服务器启动失败：${err?.message ?? String(err)}`);
    });

    // 注册客户端到扩展上下文：扩展停用时自动 dispose（等价于 stop）
    context.subscriptions.push(client);
}

/** 停用扩展：停止语言客户端（发送 shutdown/exit 并等待子进程退出）。 */
export async function deactivate(): Promise<void> {
    if (client) {
        await client.stop();
    }
}
