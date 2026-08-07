// esbuild 打包脚本：把 src/extension.ts 及其全部依赖（含 vscode-languageclient）
// 打包成单个 out/extension.js，供 VSCode 扩展宿主直接加载。
//
// 说明：
// - external: ['vscode'] —— vscode 模块由扩展宿主提供，不打包；
// - 其余依赖（vscode-languageclient / vscode-jsonrpc 等）全部打进产物，
//   因此打包 vsix 时无需携带 node_modules，产物干净且启动更快。

import * as esbuild from 'esbuild';

await esbuild.build({
    entryPoints: ['src/extension.ts'],   // 入口：TypeScript 源码
    outfile: 'out/extension.js',         // 产物：扩展主文件（package.json main 指向）
    bundle: true,                        // 打包所有依赖
    format: 'cjs',                       // CommonJS（VSCode 扩展宿主要求）
    platform: 'node',                    // 运行在 Node 环境
    target: 'es2020',                    // 与 tsconfig 的 target 保持一致
    external: ['vscode'],                // vscode API 由宿主注入
    sourcemap: false,                    // 发布产物不带 sourcemap（调试用 tsc -w）
    logLevel: 'info',
});

console.log('✔ esbuild 打包完成：out/extension.js');
