//! tie 语言服务器 VSCode 客户端（最小实现，零 npm 依赖）。
//!
//! 职责：
//! - 启动 tie 语言服务器子进程（默认 `tie --lsp`，可经 tie.lsp.command 配置）；
//! - 与服务器进行 JSON-RPC 2.0 over stdio 通信（Content-Length 分帧）；
//! - initialize 握手后，把打开/修改/关闭的 .tie 文档同步给服务器；
//! - 接收 publishDiagnostics 推送 → 显示在「问题」面板；
//! - 注册 hover provider：请求服务器 hover → 显示函数签名等信息。
//!
//! 说明：与 tie-lsp 的协议字段完全对应（didOpen/didChange/didClose/hover/publishDiagnostics）。

const vscode = require('vscode');
const { spawn } = require('child_process');

/** @type {vscode.OutputChannel} 服务器日志输出通道（「输出 → tie」） */
let logChannel;
/** @type {vscode.DiagnosticCollection} 诊断集合（问题面板数据源） */
let diagnostics;
/** @type {import('child_process').ChildProcessWithoutNullStreams | null} 服务器进程 */
let serverProcess = null;
/** @type {NodeJS.WritableStream | null} 服务器 stdin（分帧写入） */
let serverStdin = null;
/** 待解析帧的缓冲（Content-Length 头 + JSON 体） */
let recvBuffer = '';
/** 请求 id → 等待中的 resolve 回调（hover 等请求-响应） */
const pendingRequests = new Map();
/** 递增请求 id */
let nextRequestId = 1;

/** 激活：注册语言支持（激活事件 onLanguage:tie 触发）。 */
function activate(context) {
    logChannel = vscode.window.createOutputChannel('tie');
    diagnostics = vscode.languages.createDiagnosticCollection('tie');

    // 打开 / 修改 / 关闭文档 → 同步给服务器（全量同步，与服务器 textDocumentSync=1 对应）
    context.subscriptions.push(
        vscode.workspace.onDidOpenTextDocument((doc) => { if (isTie(doc)) sendDidOpen(doc); }),
        vscode.workspace.onDidChangeTextDocument((e) => { if (isTie(e.document)) sendDidChange(e.document); }),
        vscode.workspace.onDidCloseTextDocument((doc) => { if (isTie(doc)) sendDidClose(doc); })
    );

    // hover provider：请求服务器 textDocument/hover
    context.subscriptions.push(
        vscode.languages.registerHoverProvider('tie', {
            async provideHover(document, position) {
                const server = getServer();
                if (!server) return null;
                const params = {
                    textDocument: { uri: document.uri.toString() },
                    position: { line: position.line, character: position.character }
                };
                const result = await request('textDocument/hover', params);
                if (!result || !result.contents) return null;
                // 服务器返回 markdown 内容
                const md = typeof result.contents === 'string'
                    ? result.contents
                    : (result.contents.value ?? '');
                return new vscode.Hover(new vscode.MarkdownString(md));
            }
        })
    );

    // 先启动服务器，把已打开的 tie 文档补发 didOpen（激活可能晚于文档打开）
    ensureServer();
    for (const doc of vscode.workspace.textDocuments) {
        if (isTie(doc)) sendDidOpen(doc);
    }
}

/** 停用：发 shutdown/exit 通知并结束进程。 */
function deactivate() {
    if (!serverProcess) return;
    try {
        sendRaw({ jsonrpc: '2.0', method: 'shutdown' });
        sendRaw({ jsonrpc: '2.0', method: 'exit' });
    } catch { /* 进程可能已退出 */ }
    serverProcess.kill();
}

/** 判断文档是否为 tie 语言。 */
function isTie(doc) {
    return doc.languageId === 'tie' || doc.fileName.endsWith('.tie');
}

/** 惰性获取服务器（未启动则启动并完成 initialize 握手）。 */
function getServer() {
    if (!serverProcess) ensureServer();
    return serverProcess ? serverStdin : null;
}

/** 启动 tie 语言服务器子进程并完成 initialize 握手。 */
function ensureServer() {
    if (serverProcess) return;

    // 命令：tie.lsp.command 配置（默认 ['tie', '--lsp']）
    const cmd = vscode.workspace.getConfiguration('tie').get('lsp.command', ['tie', '--lsp']);
    if (!Array.isArray(cmd) || cmd.length === 0) {
        log(`错误: tie.lsp.command 配置无效: ${JSON.stringify(cmd)}`);
        return;
    }

    log(`启动语言服务器: ${cmd.join(' ')}`);
    const child = spawn(cmd[0], cmd.slice(1), { stdio: ['pipe', 'pipe', 'pipe'] });
    serverProcess = child;
    serverStdin = child.stdin;

    // stderr → 日志（服务器诊断信息）
    child.stderr.on('data', (d) => log(`[stderr] ${String(d)}`));
    child.on('error', (err) => {
        log(`语言服务器启动失败: ${err.message}（请确认 tie 已构建且 tie.lsp.command 路径正确）`);
        serverProcess = null;
        serverStdin = null;
    });
    child.on('exit', (code) => {
        log(`语言服务器已退出，code=${code}`);
        serverProcess = null;
        serverStdin = null;
        diagnostics.clear();
        pendingRequests.clear();
    });

    // stdout → 分帧解析
    child.stdout.on('data', (chunk) => { recvBuffer += chunk.toString('utf8'); drainBuffer(); });

    // 完成 initialize 握手
    request('initialize', { processId: process.pid, rootUri: null, capabilities: {} })
        .then(() => log('initialize 完成，服务器就绪'))
        .catch((err) => log(`initialize 失败: ${err}`));
}

/** 解析 stdout 缓冲中的完整帧（Content-Length 头 + JSON 体），逐条分发。 */
function drainBuffer() {
    // 标准帧格式: "Content-Length: <n>\r\n\r\n<body>"
    for (;;) {
        const headerEnd = recvBuffer.indexOf('\r\n\r\n');
        if (headerEnd < 0) break;
        const header = recvBuffer.slice(0, headerEnd);
        const m = /Content-Length:\s*(\d+)/i.exec(header);
        if (!m) { recvBuffer = recvBuffer.slice(headerEnd + 4); continue; }
        const len = Number(m[1]);
        const bodyStart = headerEnd + 4;
        if (recvBuffer.length < bodyStart + len) break; // 帧未收全
        const body = recvBuffer.slice(bodyStart, bodyStart + len);
        recvBuffer = recvBuffer.slice(bodyStart + len);
        handleMessage(JSON.parse(body));
    }
}

/** 分发一条 JSON-RPC 消息：请求/响应/通知。 */
function handleMessage(msg) {
    // 响应（id 命中等待中的请求）
    if (msg.id !== undefined && pendingRequests.has(msg.id)) {
        const { resolve, reject } = pendingRequests.get(msg.id);
        pendingRequests.delete(msg.id);
        if (msg.error) reject(new Error(msg.error.message || 'LSP 请求失败'));
        else resolve(msg.result);
        return;
    }
    // 通知
    switch (msg.method) {
        case 'textDocument/publishDiagnostics':
            applyDiagnostics(msg.params);
            break;
        default:
            break; // 其余通知忽略
    }
}

/** 把服务器推送的诊断写入问题面板。 */
function applyDiagnostics(params) {
    const uri = vscode.Uri.parse(params.uri);
    const items = (params.diagnostics || []).map((d) => {
        const r = d.range;
        return new vscode.Diagnostic(
            new vscode.Range(r.start.line, r.start.character, r.end.line, r.end.character),
            d.message,
            severityOf(d.severity)
        );
    });
    diagnostics.set(uri, items);
}

/** LSP severity(1=Error,2=Warning,3=Info,4=Hint) → vscode.DiagnosticSeverity。 */
function severityOf(s) {
    switch (s) {
        case 1: return vscode.DiagnosticSeverity.Error;
        case 2: return vscode.DiagnosticSeverity.Warning;
        case 3: return vscode.DiagnosticSeverity.Information;
        default: return vscode.DiagnosticSeverity.Hint;
    }
}

/** 发送请求并等待响应（promise）。 */
function request(method, params) {
    const id = nextRequestId++;
    return new Promise((resolve, reject) => {
        pendingRequests.set(id, { resolve, reject });
        sendRaw({ jsonrpc: '2.0', id, method, params });
    });
}

/** 分帧写入一条消息（请求/响应/通知统一入口）。 */
function sendRaw(obj) {
    if (!serverStdin) throw new Error('语言服务器未就绪');
    const body = JSON.stringify(obj);
    serverStdin.write(`Content-Length: ${Buffer.byteLength(body, 'utf8')}\r\n\r\n${body}`);
}

/** 文档打开 → didOpen 通知（全量文本）。 */
function sendDidOpen(doc) {
    try {
        sendRaw({
            jsonrpc: '2.0',
            method: 'textDocument/didOpen',
            params: {
                textDocument: { uri: doc.uri.toString(), languageId: 'tie', version: doc.version, text: doc.getText() }
            }
        });
    } catch { /* 服务器未就绪时忽略 */ }
}

/** 文档修改 → didChange 通知（全量替换，与服务器 textDocumentSync=1 对应）。 */
function sendDidChange(doc) {
    try {
        sendRaw({
            jsonrpc: '2.0',
            method: 'textDocument/didChange',
            params: {
                textDocument: { uri: doc.uri.toString(), version: doc.version },
                contentChanges: [{ text: doc.getText() }]
            }
        });
    } catch { /* 忽略 */ }
}

/** 文档关闭 → didClose 通知。 */
function sendDidClose(doc) {
    try {
        sendRaw({ jsonrpc: '2.0', method: 'textDocument/didClose', params: { textDocument: { uri: doc.uri.toString() } } });
    } catch { /* 忽略 */ }
}

/** 写日志到输出通道。 */
function log(msg) {
    if (logChannel) logChannel.appendLine(`[${new Date().toLocaleTimeString()}] ${msg}`);
}

module.exports = { activate, deactivate };
