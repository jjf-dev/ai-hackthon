# Asterinas Ext2 Xfstests 修复工作

## Background

### Xfstests

xfstests 是当前最为复杂的文件系统测试系统，提供了一系列复杂的测试，测试文件系统的完备性。

在当前的 Asterinas 下，ext2 文件系统在重构的过程中，存在若干失败的测试。初始状态：61 个 generic 测试失败。

主要问题分为：
- **Ext2 实现 Bug**：EOF 扩展、文件大小检查、fallocate 语义
- **VFS 层 Bug**：Dentry negative cache、目录迭代 cookie
- **时间戳/权限 Bug**：atime 更新、utimens 权限检查、负时间戳支持
- **缺失功能**：系统调用（AIO、splice、swap）、块设备 ioctl、挂载传播

## Overview

核心工作如下：
1. 探索了一套 spec-driven 的文件系统 bug 修复工作流，将 Linux 作为 source of truth，确认正确的逻辑，撰写 spec，实现逻辑并且补全测试
2. 探索了 Agent 在复杂逻辑和大量日志下的分析和定位问题能力
3. 探索了 Agent 在日常开发中的辅助能力和不足

**结果**：找出了所有 bug 对应的原因，修复了 19个

## Solution

建立 spec-driven 的 bug 修复工作流：

```
Error Analysis → Bug Classification → Spec Writing → Implementation → Verification
```

## Workflow

完整的 bug 修复工作流分为三个阶段：

### 1. Error Analysis
使用 Agent 分析 xfstests 失败日志，交叉对比测试逻辑与代码实现，生成结构化诊断报告。

定义了专用 skill，用于将 error log 和 xfstests 的逻辑交叉对比，最终生成 report。

**报告内容：**
1. **测试逻辑**：3 句话概括测试目的
2. **Linux 参考**：调用路径和表现（引用源码位置）
3. **诊断逻辑**：reason + call path + evidence
4. **修复建议**：具体到文件和函数

### 2. Bug Fix
采用 spec-driven 方式修复：
1. **Brainstorm**：Agent 交互式向 user 提问，确认实现细节，将细节持久化成 PRD
2. **Write Spec**：编写形式化规约 PRECONDITIONS + SUCCESS POSTCONDITIONS + FAILURE POSTCONDITIONS + INVARIANTS
3. **Implementation**：基于 spec 生成代码，与 Linux 逻辑验证
4. **Rerun xfstests**：验证修复效果，根据反馈迭代

### 3. Code Review
使用 `kernel-architecture-audit` skill 检查修复的代码，保证不是补丁式的修复。

**审查重点：**
- **抽象边界完整性**：VFS 层不能包含具体文件系统的知识
- **所有权边界**：通用层不能直接修改文件系统私有状态
- **Linux 对齐**：与 Linux 内核的分层和调度模型保持一致
- **避免测试驱动的 hack**：不接受"仅为通过测试"的特殊分支
- **接口设计**：优先通过 trait/接口重构解决问题，而非堆叠条件分支

**检查清单：**
- 是否是为通过特定测试而注入的补丁式修复？
- VFS 或其他通用层是否触及具体文件系统实现？
- 是否破坏了未来文件系统集成的架构一致性？
- 设计是否与 Linux 的标准分层和调度模型对齐？
- 是否应该通过 trait/接口重构而非分支堆叠来解决？
- 当前补丁实际暴露了什么缺失的抽象？


## Result

### 分析成果
- 问题分类（9 大类）
- 问题对应的 test（61 个）
- 依赖关系（9 层优先级）
- 所有 error 均可以分析出具体问题，包括较为复杂的 race 问题（如 generic/080: pwrite + mmap write）

| Category | Count | Percentage |
|----------|-------|------------|
| Ext2 Specific Bugs | 8 | 13.1% |
| VFS Layer Bugs | 4 | 6.6% |
| Timestamp/Permission | 9 | 14.8% |
| Missing Syscalls | 16 | 26.2% |
| Block Device/ioctl | 5 | 8.2% |
| Mount Propagation | 5 | 8.2% |
| Procfs/Sysctl | 7 | 11.5% |
| Test Environment | 5 | 8.2% |
| Other / Unclassified | 2 | 3.2% |
| **Total** | **61** | **100%** |

### 修复成果
- **已修复**：19 个测试
- **代码变更**：23 个文件，~1200 行新增代码

### 典型修复
1. **generic/011** - Dentry 缓存一致性：VFS 层在失败时未回滚 dentry 状态
2. **generic/466** - 文件大小检查：写入前未检查 `s_maxbytes`，错误码不正确
3. **generic/257** - 目录迭代：`d_off` 返回当前偏移而非下一条目位置

### 功能实现
1. **generic/015** - splice syscall

## SwiftIndex

为 Agent 开发的轻量级 Rust 代码索引工具，核心创新是**紧凑检索 + 渐进扩展**：默认返回最小有用结果，Agent 可按需扩展。

**特性**：
- 基于 SQLite + FTS5 的本地索引
- 多信号重排序（词法、结构、Git、关系）
- 置信度感知的动态 top-k 选择
- 支持符号查找、文件搜索、代码大纲、邻居发现等查询接口

详见 [SwiftIndex.md](./SwiftIndex.md)

## Agent 使用

### 核心 Skills

| Skill | 用途 |
|-------|------|
| `brainstorm` | 交互式提问，确认实现细节 |
| `spec-creator` | 编写形式化规约 |
| `spec-linux-validator` | 验证 spec 与 Linux 逻辑一致性 |
| `xfstests-failure-analysis` | 分析测试失败日志，生成诊断报告 |
| `xfstests-fix` | 基于诊断报告修复 bug |
| `kernel-architecture-audit` | 检查代码架构合规性 |





