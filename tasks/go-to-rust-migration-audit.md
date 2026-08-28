# Go → Rust 迁移最终审计

> 本文件是迁移完成后的唯一台账，不是实时 PR 看板。实时 review、CI 和
> merge 状态以 GitHub PR 与 GitHub Actions 为准；不再按 PR 追加重复章节。

## 结论

本提交代表的仓库状态已经退休旧 Go backend：生产 server、daemon、CLI、
migration runner、backfill、构建、测试、容器、安装器和发布链均以 Rust 为唯一
实现。旧 `server/`、全部受版本控制的 `.go` 文件、Go modules、sqlc 配置及 Go
构建/测试脚本已删除。

最终删除变更只有在对应 PR 的全部必需 GitHub Actions 成功、有效 review
comments 全部处理且 head SHA 未变化后才允许合并。Agent 不在本机运行编译、
测试、格式化或 Docker 构建，也不以本地结果替代 GitHub 云端门禁。

## 最终仓库布局

- `server-rs/`：唯一生产 backend、daemon、CLI、migration runner 与 backfills。
- `migrations/`：Rust migration runner 使用的唯一生产 SQL migration 目录。
- `server-rs/crates/cordy-service/assets/`：内嵌 agents 与 skills。
- `server-rs/crates/cordy-handler/assets/reserved_slugs.json`：reserved slug 单一来源。
- `bin/`：开发构建产物目录；发布产物由 Rust release workflows 生成。
- `AGENTS.md`：Agent 规则单一来源；`CLAUDE.md` 只保留指向该文件的兼容引用。

## 已完成的退休范围

- [x] Rust production assemblies 已接管 HTTP server、daemon、CLI、migration 与 backfill。
- [x] Docker、Compose、Helm、installers、desktop bundle、CI 与 release workflows 不再构建或分发 Go binary。
- [x] 828 个 SQL migration 文件迁至根 `migrations/`，Rust runner 与镜像路径同步更新。
- [x] 内嵌 agent、skill 与 reserved-slug assets 迁入 Rust crate 所属目录。
- [x] route inventory 使用 `server-rs/route-contract/routes.tsv`，不再生成或读取 Go route inventory。
- [x] CLI 与镜像元数据删除 Go version 字段和环境变量。
- [x] Go test、build、migrate、sqlc targets 与脚本退出产品和 CI 链。
- [x] 旧 `server/` 整树、Go modules、sqlc queries/config 和全部 Go 源码删除。
- [x] 用户与运维文档改为 Rust binary、根 migrations 和 GitHub Actions 路径。

## Review 与前置验证证据

迁移 stack 的剩余有效 findings 已集中在 PR #593 修复：

- 迁移审计共盘点 299 条未解决 P1/P2；重复、过时或已被后续 stack 取代的
  261 条先行收口，剩余 38 条有效 comments 全部修复、回复并 resolved。
- 114 个旧 stacked PR 已按“重复/被取代/已集中修复”收口，不删除其远端分支。
- PR #593 的 head `c3056aab16497034f651d51db0fe47f6314ec2e7` 在 GitHub Actions
  run `33172199024` 通过 Rust quality/tests/audit、Linux production image、
  Windows/macOS、frontend、deployment contracts 与聚合门禁。
- PR #593 以 merge commit
  `488b8cf241f904c0be4aef869c2df1d6a1095aad` 合入 `main`；第二父提交与
  已验证 head 完全一致。

## 当前提交的静态退出条件

以下检查只验证仓库结构，不替代最终 PR 的 GitHub Actions：

```text
git ls-files '*.go'                         => 0
git ls-files 'server/**'                    => 0
tracked go.mod / go.sum / sqlc config       => 0
Go build/test/release executable references => 0
git diff --check                            => pass
conflict-marker scan                        => pass
```

协议测试、迁移 SQL 注释或历史 Git 记录可以描述曾经需要兼容的 Go 行为；这些
历史文字不属于可执行依赖。不得重新引入 Go toolchain、Go source、Go runtime
binary、双 backend build 或 Go CI gate。

## 最终 PR 合并门

1. GitHub Actions 必须自动触发，并完成适用的 Rust format/check/Clippy/tests、
   production image、deployment contracts、audit、installers 与平台矩阵。
2. 任何代码失败必须修复并由新 head 的 Actions 重新验证；不得以环境或本地磁盘
   为由跳过云端失败。
3. Codex/人工 review 新增的有效 P1/P2 必须回复、修复并 resolved。
4. 合并前再次核对 base=`main`、精确 head SHA、mergeability 与 required checks；
   使用仓库规定的 merge-commit 方法，并核对 merge commit 的父提交。
5. 合并后重新审计 `main`：零受版本控制 Go 源码、零旧 backend 入口、零未解决
   有效 migration comments。

历史逐切片实现细节保留在 Git 历史以及已合并/关闭的 PR 中，不再复制回本文件。
